//! 设备准备/停止的期限与无进展重查门；新证据不受旧轮询退避限制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Due,
    Waiting,
    Expired,
}

pub(crate) fn deadline(retry: Option<u64>, phase_end: Option<u64>) -> Option<u64> {
    match (retry, phase_end) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(at), None) | (None, Some(at)) => Some(at),
        (None, None) => None,
    }
}
pub(crate) fn gate(
    now: u64,
    retry: Option<u64>,
    phase_end: Option<u64>,
    new_evidence: bool,
) -> Gate {
    if phase_end.is_some_and(|end| now >= end) {
        Gate::Expired
    } else if !new_evidence && retry.is_some_and(|at| now < at) {
        Gate::Waiting
    } else {
        Gate::Due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timely_stopped_report_bypasses_old_poll_backoff() {
        // Stop 在 80 收到 Stopping，重查落在 110；81 的真实报告须能在 100 前转交。
        assert_eq!(gate(81, Some(110), Some(100), false), Gate::Waiting);
        assert_eq!(gate(81, Some(110), Some(100), true), Gate::Due);
        assert_eq!(gate(100, Some(110), Some(100), true), Gate::Expired);
    }
    #[test]
    fn idle_polling_publishes_phase_deadline_even_without_dispatcher_call() {
        // Preparing 在 90 返回，下一次轮询 120；100 必须有独立 Runtime 期限。
        assert_eq!(deadline(Some(120), Some(100)), Some(100));
        assert_eq!(gate(100, Some(120), Some(100), false), Gate::Expired);
        assert_eq!(deadline(Some(95), Some(100)), Some(95));
        assert_eq!(deadline(None, None), None);
    }
}
