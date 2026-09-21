//! 出站调用的纯状态推进；运输 owner 保存在调用记录中。

#[cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutboundStage {
    Ready,
    WaitingWritable,
    Sent,
    Terminal,
}

#[cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]
impl OutboundStage {
    pub(crate) const fn can_attempt(self) -> bool {
        matches!(self, Self::Ready)
    }

    pub(crate) fn blocked(&mut self) -> bool {
        if *self != Self::Ready {
            return false;
        }
        *self = Self::WaitingWritable;
        true
    }

    pub(crate) fn writable(&mut self) -> bool {
        if *self != Self::WaitingWritable {
            return false;
        }
        *self = Self::Ready;
        true
    }

    pub(crate) fn sent(&mut self) -> bool {
        if *self != Self::Ready {
            return false;
        }
        *self = Self::Sent;
        true
    }

    pub(crate) fn finish(&mut self) -> bool {
        if *self == Self::Terminal {
            return false;
        }
        *self = Self::Terminal;
        true
    }
}

/// 有界扫描只保存游标；扫描中的新工作要求下一轮复查，不倒退当前进度。
#[cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]
#[derive(Default)]
pub(crate) struct Sweep {
    pub(crate) cursor: u64,
    pub(crate) active: bool,
    dirty: bool,
}

#[cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]
impl Sweep {
    pub(crate) fn request(&mut self) {
        if self.active {
            self.dirty = true;
        } else {
            self.active = true;
        }
    }

    pub(crate) fn candidate(&mut self, next: Option<u64>) -> Option<u64> {
        if !self.active {
            return None;
        }
        if let Some(key) = next {
            self.cursor = key;
            Some(key)
        } else {
            self.cursor = 0;
            self.active = core::mem::take(&mut self.dirty);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OutboundStage, Sweep};

    #[test]
    fn full_mailbox_retries_only_after_writable() {
        let mut stage = OutboundStage::Ready;
        assert!(stage.blocked());
        assert!(!stage.can_attempt());
        assert!(!stage.blocked());
        assert!(stage.writable());
        assert!(stage.can_attempt());
        assert!(stage.sent());
        assert!(!stage.writable());
        assert!(!stage.sent());
    }

    #[test]
    fn terminal_is_idempotent_from_every_live_stage() {
        for mut stage in [
            OutboundStage::Ready,
            OutboundStage::WaitingWritable,
            OutboundStage::Sent,
        ] {
            assert!(stage.finish());
            assert_eq!(stage, OutboundStage::Terminal);
            assert!(!stage.finish());
            assert!(!stage.can_attempt());
        }
    }

    #[test]
    fn scan_finishes_without_work_and_restarts_for_earlier_changes() {
        let mut sweep = Sweep::default();
        sweep.request();
        assert_eq!(sweep.candidate(Some(2)), Some(2));
        sweep.request();
        assert_eq!(sweep.cursor, 2);
        assert_eq!(sweep.candidate(Some(4)), Some(4));
        assert_eq!(sweep.candidate(None), None);
        assert!(sweep.active);
        assert_eq!(sweep.cursor, 0);
        assert_eq!(sweep.candidate(Some(1)), Some(1));
        assert_eq!(sweep.candidate(None), None);
        assert!(!sweep.active);
        sweep.request();
        assert!(sweep.active);
        assert_eq!(sweep.cursor, 0);
    }
}
