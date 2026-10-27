//! 设备观察的序号合同；通知只是 pending 的投影，不能代替 Query/Ack。
use crate::Error;
use librpc::bytes::{Reader, Writer};

pub const INITIAL: u64 = 1;
pub const CHANGED: u64 = 2;
pub const TERMINAL: u64 = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Reason {
    Active = 0,
    Cancelled = 1,
    Revoked = 2,
    Released = 3,
    Exhausted = 4,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub sequence: u64,
    pub pending: u64,
    pub reason: Reason,
    /// Scope 观察不携带绑定；Binding 观察与序号在同一 actor 中取样。
    pub binding: Option<crate::binding::Snapshot>,
}
impl Info {
    pub const ENCODED_LEN: usize = 60;
    pub fn terminal(self) -> bool {
        self.reason != Reason::Active
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < Self::ENCODED_LEN {
            return Err(Error::Invalid);
        }
        let mut w = Writer::new(out);
        w.u64(self.sequence);
        w.u64(self.pending);
        w.u32(self.reason as u32);
        w.u32(u32::from(self.binding.is_some()));
        if let Some(binding) = self.binding {
            crate::protocol::encode_snapshot(binding, &mut out[24..Self::ENCODED_LEN])?;
        } else {
            out[24..Self::ENCODED_LEN].fill(0);
        }
        Ok(Self::ENCODED_LEN)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let sequence = r.u64()?;
        let pending = r.u64()?;
        let reason = match r.u32()? {
            0 => Reason::Active,
            1 => Reason::Cancelled,
            2 => Reason::Revoked,
            3 => Reason::Released,
            4 => Reason::Exhausted,
            _ => return Err(Error::Invalid),
        };
        let binding = match r.u32()? {
            0 => {
                if r.bytes(36)?.iter().any(|byte| *byte != 0) {
                    return Err(Error::Invalid);
                }
                None
            }
            1 => Some(crate::protocol::decode_snapshot(r.bytes(36)?)?),
            _ => return Err(Error::Invalid),
        };
        if sequence == 0
            || pending & !(INITIAL | CHANGED | TERMINAL) != 0
            || (reason == Reason::Active && (sequence == u64::MAX || pending & TERMINAL != 0))
            || (reason != Reason::Active && pending != 0 && pending & TERMINAL == 0)
            || (reason != Reason::Active && sequence == 1)
            || (reason == Reason::Exhausted && sequence != u64::MAX)
        {
            return Err(Error::Invalid);
        }
        r.finish()?;
        Ok(Self {
            sequence,
            pending,
            reason,
            binding,
        })
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Sequence(Info);
impl Sequence {
    pub(crate) fn new() -> Self {
        Self(Info {
            sequence: 1,
            pending: INITIAL,
            reason: Reason::Active,
            binding: None,
        })
    }
    pub(crate) fn info(self) -> Info {
        self.0
    }
    pub(crate) fn change(&mut self) {
        if self.0.terminal() {
            return;
        }
        if self.0.sequence == u64::MAX - 1 {
            self.terminate(Reason::Exhausted);
        } else {
            self.0.sequence += 1;
            self.0.pending |= CHANGED;
        }
    }
    pub(crate) fn terminate(&mut self, reason: Reason) {
        if self.0.terminal() {
            return;
        }
        // 活动序号最多 MAX-1，最后一个序号专用于终态。
        self.0.sequence += 1;
        self.0.pending |= TERMINAL;
        self.0.reason = reason;
    }
    pub(crate) fn ack(&mut self, sequence: u64) -> Result<Info, Error> {
        if sequence == 0 || sequence > self.0.sequence {
            return Err(Error::Invalid);
        }
        if sequence == self.0.sequence {
            self.0.pending = 0;
        }
        Ok(self.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_ack_cannot_hide_a_later_change() {
        let mut state = Sequence::new();
        let old = state.info();
        state.change();
        assert_eq!(state.ack(old.sequence).unwrap(), state.info());
        assert_eq!(state.ack(state.info().sequence + 1), Err(Error::Invalid));
        assert_eq!(state.ack(state.info().sequence).unwrap().pending, 0);
    }
    #[test]
    fn exhaustion_has_a_final_nonwrapping_receipt() {
        let mut state = Sequence(Info {
            sequence: u64::MAX - 1,
            pending: 0,
            reason: Reason::Active,
            binding: None,
        });
        state.change();
        let terminal = state.info();
        assert_eq!(terminal.sequence, u64::MAX);
        assert_eq!(terminal.reason, Reason::Exhausted);
        state.change();
        state.terminate(Reason::Cancelled);
        assert_eq!(state.info(), terminal);
        let mut bytes = [0; Info::ENCODED_LEN];
        terminal.encode(&mut bytes).unwrap();
        assert_eq!(Info::decode(&bytes).unwrap(), terminal);
        assert_eq!(state.ack(u64::MAX).unwrap().pending, 0);
    }

    #[test]
    fn binding_watch_pairs_the_original_round_and_terminal_snapshot() {
        use crate::{
            binding::{Phase, Snapshot, Subject},
            identity::{BindingRef, DeviceId},
        };
        let reference = BindingRef::new(DeviceId::new(8, 3).unwrap(), 9).unwrap();
        let info = Info {
            sequence: 5,
            pending: TERMINAL,
            reason: Reason::Released,
            binding: Some(Snapshot {
                reference,
                subject: Subject::from_authenticated_context(27).unwrap(),
                phase: Phase::Released,
            }),
        };
        let mut bytes = [0; Info::ENCODED_LEN];
        assert_eq!(info.encode(&mut bytes).unwrap(), 60);
        assert_eq!(&bytes[24..32], &8u64.to_le_bytes());
        assert_eq!(&bytes[40..48], &9u64.to_le_bytes());
        assert_eq!(Info::decode(&bytes).unwrap(), info);
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Info::decode(&bytes), Err(Error::Invalid));
    }
}
