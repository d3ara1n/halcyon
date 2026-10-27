//! PLIC 已领取事件的消费状态；硬件访问和串行锁由内核控制器拥有。
#![no_std]

use erhino_shared::{call::SystemCallError as Error, irq::IrqToken};

#[derive(Debug)]
pub struct Service {
    generation: u64,
    sequence: u64,
    in_service: bool,
    taken: bool,
    closed: bool,
}
impl Service {
    pub const fn new(generation: u64) -> Self {
        assert!(generation != 0, "IRQ generation must be nonzero");
        Self {
            generation,
            sequence: 0,
            in_service: false,
            taken: false,
            closed: false,
        }
    }
    /// 硬件已经 claim。序号耗尽时调用者必须 complete 此次领取并封源。
    pub fn claim(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::ObjectClosed);
        }
        assert!(
            !self.in_service,
            "PLIC gateway delivered an outstanding source twice"
        );
        self.sequence = self.sequence.checked_add(1).ok_or(Error::ReachLimit)?;
        self.in_service = true;
        self.taken = false;
        Ok(())
    }
    pub fn take(&mut self) -> Result<IrqToken, Error> {
        if self.closed {
            return Err(Error::ObjectClosed);
        }
        if !self.readable() {
            return Err(Error::ObjectNotAvailable);
        }
        self.taken = true;
        Ok(IrqToken {
            generation: self.generation,
            sequence: self.sequence,
        })
    }
    pub fn ack(&mut self, token: IrqToken) -> Result<(), Error> {
        if self.closed {
            return Err(Error::ObjectClosed);
        }
        if !self.in_service
            || !self.taken
            || token.generation != self.generation
            || token.sequence != self.sequence
        {
            return Err(Error::IllegalArgument);
        }
        self.in_service = false;
        self.taken = false;
        Ok(())
    }
    /// 返回是否仍有硬件领取必须在禁源前 complete。
    pub fn close(&mut self) -> bool {
        self.closed = true;
        self.taken = false;
        core::mem::replace(&mut self.in_service, false)
    }
    pub const fn closed(&self) -> bool {
        self.closed
    }
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    pub const fn readable(&self) -> bool {
        self.in_service && !self.taken && !self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_take_and_exactly_one_ack_per_claim() {
        let mut state = Service::new(7);
        assert_eq!(state.take(), Err(Error::ObjectNotAvailable));
        state.claim().unwrap();
        let token = state.take().unwrap();
        assert!(!state.readable());
        assert_eq!(state.take(), Err(Error::ObjectNotAvailable));
        state.ack(token).unwrap();
        assert_eq!(state.ack(token), Err(Error::IllegalArgument));
        state.claim().unwrap();
        assert_eq!(state.ack(token), Err(Error::IllegalArgument));
        let next = state.take().unwrap();
        assert_ne!(token, next);
        state.ack(next).unwrap();
    }
    #[test]
    fn revoke_completes_even_if_not_taken_and_old_token_never_crosses_use() {
        for taken in [false, true] {
            let mut old = Service::new(8);
            old.claim().unwrap();
            let token = if taken {
                old.take().unwrap()
            } else {
                IrqToken {
                    generation: 8,
                    sequence: 1,
                }
            };
            assert!(old.close());
            assert!(!old.close());
            assert_eq!(old.ack(token), Err(Error::ObjectClosed));
            assert_eq!(old.take(), Err(Error::ObjectClosed));
            let mut new = Service::new(9);
            new.claim().unwrap();
            let current = new.take().unwrap();
            assert_eq!(new.ack(token), Err(Error::IllegalArgument));
            new.ack(current).unwrap();
        }
    }
    #[test]
    fn last_sequence_is_usable_but_never_wraps() {
        let mut state = Service::new(10);
        state.sequence = u64::MAX - 1;
        state.claim().unwrap();
        let token = state.take().unwrap();
        assert_eq!(token.sequence, u64::MAX);
        state.ack(token).unwrap();
        assert_eq!(state.claim(), Err(Error::ReachLimit));
        // 失败 claim 的硬件 complete 归调用方；状态不伪造第二份 in-service。
        assert!(!state.close());
    }
}
