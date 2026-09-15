//! 监督机器与驱动器之间的完整观察契约。请求身份不复用，期限在请求建立时冻结。

use crate::{SupervisionCause, SupervisionPolicy};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    time::Deadline,
};
use libsrv::runtime::{Input, Requests, SourceId, SourceKind};

static NEXT_OBSERVATION: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    pub id: u64,
    pub control: Handle,
    pub signals: ObjectSignals,
    pub deadline: Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationResult {
    Ready(ObjectSignals),
    TimedOut,
    SourceError(SystemCallError),
}

#[derive(Debug, Default)]
pub(crate) struct ObservationState {
    pending: Option<Observation>,
    timeouts: u32,
}

impl ObservationState {
    pub fn request(
        &mut self,
        control: Handle,
        signals: ObjectSignals,
        now: u64,
        policy: SupervisionPolicy,
    ) -> Result<Observation, SupervisionCause> {
        if let Some(request) = self.pending {
            return Ok(request);
        }
        if self.timeouts >= policy.wait_attempts {
            return Err(SupervisionCause::Timeout);
        }
        let duration = policy
            .wait_timeout_ms
            .checked_mul(1_000_000)
            .and_then(|delta| now.checked_add(delta))
            .ok_or(SupervisionCause::System(SystemCallError::ReachLimit))?;
        let id = NEXT_OBSERVATION
            .allocate()
            .ok_or(SupervisionCause::System(SystemCallError::ReachLimit))?;
        let request = Observation {
            id,
            control,
            signals,
            deadline: Deadline::at(duration),
        };
        self.pending = Some(request);
        Ok(request)
    }

    /// 错身份与提前超时不改变当前请求；有效完成只消费一次。
    pub fn accept(
        &mut self,
        request: Observation,
        result: ObservationResult,
        now: u64,
    ) -> Result<bool, SupervisionCause> {
        if self.pending != Some(request) {
            return Err(SupervisionCause::System(SystemCallError::IllegalArgument));
        }
        let at = request
            .deadline
            .instant()
            .expect("observation deadline is valid")
            .expect("observation deadline is finite");
        if matches!(result, ObservationResult::TimedOut) && now < at {
            return Err(SupervisionCause::System(SystemCallError::IllegalArgument));
        }
        if let ObservationResult::Ready(signals) = result
            && !signals.intersects(request.signals)
        {
            return Err(SupervisionCause::System(SystemCallError::IllegalArgument));
        }
        self.pending = None;
        match result {
            ObservationResult::SourceError(error) => Err(SupervisionCause::System(error)),
            // Ready 已由等待裁决选中；不能被随后注销/调度的耗时推翻。
            ObservationResult::Ready(_) => {
                self.timeouts = 0;
                Ok(true)
            }
            ObservationResult::TimedOut => {
                self.timeouts = self.timeouts.saturating_add(1);
                Ok(false)
            }
        }
    }

    pub fn pending(&self) -> Option<Observation> {
        self.pending
    }

    pub fn renew(&mut self) {
        self.timeouts = 0;
        self.pending = None;
    }
}

/// Runtime 侧观察租约。先收到注销回执，再将观察结果交给机器推进关闭。
#[derive(Debug, Default)]
pub struct ObservationSlot {
    request: Option<Observation>,
    source: Option<SourceId>,
    installing: bool,
    removing: bool,
    result: Option<ObservationResult>,
}

impl ObservationSlot {
    pub fn begin<T>(
        &mut self,
        request: Observation,
        requests: &mut Requests<T>,
    ) -> Result<(), SystemCallError> {
        if self.request.is_some() {
            return if self.request == Some(request) {
                Ok(())
            } else {
                Err(SystemCallError::ObjectBusy)
            };
        }
        requests.add_source(request.control, request.signals, request.id)?;
        self.request = Some(request);
        self.installing = true;
        Ok(())
    }

    pub fn registered(&mut self, kind: SourceKind, source: SourceId) {
        if self.request.is_some_and(|request| request.id == kind) {
            self.source = Some(source);
            self.installing = false;
        }
    }

    pub fn unregistered(&mut self, kind: SourceKind, source: SourceId) {
        if self.request.is_some_and(|request| request.id == kind) && self.source == Some(source) {
            self.source = None;
            self.removing = false;
        }
    }

    pub fn refused(&mut self, kind: SourceKind, error: SystemCallError) {
        if self.request.is_some_and(|request| request.id == kind) {
            self.installing = false;
            self.result = Some(ObservationResult::SourceError(error));
        }
    }

    pub fn deadline(&self) -> Deadline {
        if self.result.is_some() {
            Deadline::INFINITE
        } else {
            self.request
                .map_or(Deadline::INFINITE, |request| request.deadline)
        }
    }

    pub fn is_active(&self) -> bool {
        self.request.is_some()
    }

    /// 停止仍要撤销正在等待的来源；调用方取得回执后才可交付机器。
    pub fn cancel(&mut self) {
        if self.request.is_some() {
            self.result = Some(ObservationResult::SourceError(
                SystemCallError::ObjectClosed,
            ));
        }
    }

    pub fn poll<T>(
        &mut self,
        input: &mut Input<'_>,
        requests: &mut Requests<T>,
        now: u64,
        probe: impl FnOnce(Observation) -> Result<ObservationResult, SystemCallError>,
    ) -> Result<Option<(Observation, ObservationResult)>, SystemCallError> {
        let Some(request) = self.request else {
            return Ok(None);
        };
        while let Some(event) = input.pull() {
            if event.kind != request.id || self.source != Some(event.source) {
                continue;
            }
            if self.result.is_none() {
                self.result = Some(if event.error != 0 {
                    ObservationResult::SourceError(
                        num_traits::FromPrimitive::from_u32(event.error)
                            .unwrap_or(SystemCallError::Unknown),
                    )
                } else {
                    ObservationResult::Ready(event.observed)
                });
            }
        }
        // 到期事实由统一绝对时钟决定；迟到 timeout 不跨轮消费新请求。
        let _ = input.take_timeout();
        if self.result.is_none()
            && request
                .deadline
                .instant()
                .ok()
                .flatten()
                .is_some_and(|at| now >= at)
        {
            // 期限到达时仅做一次非阻塞裁决。Process/Job终态电平持久，
            // 即使WaitSet记录尚未取出，也不会把已就绪来源误判为超时。
            self.result = Some(probe(request).unwrap_or_else(ObservationResult::SourceError));
        }
        if self.result.is_some() && !self.installing {
            if let Some(source) = self.source {
                if !self.removing {
                    requests.remove(source)?;
                    self.removing = true;
                }
            } else {
                let result = self
                    .result
                    .take()
                    .expect("observation completion must remain owned");
                self.request = None;
                return Ok(Some((request, result)));
            }
        }
        Ok(None)
    }
}

/// 同步驱动只负责等待，机器仍验证同一个请求、期限和结果。
pub fn wait(request: Observation) -> Result<(u64, ObservationResult), SystemCallError> {
    use erhino_shared::wait::{WaitItem, WaitReason};
    let result = match rinlib::ipc::wait::wait_until(
        &[WaitItem::new(request.control, request.signals, request.id)],
        request.deadline,
    ) {
        Ok(result) if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) => {
            ObservationResult::TimedOut
        }
        Ok(result) => ObservationResult::Ready(result.observed),
        Err(error) => ObservationResult::SourceError(error),
    };
    Ok((rinlib::time::snapshot()?.now_ns, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_ready_is_not_reclassified_by_late_resume() {
        for delay in [0, 1_000_000] {
            let mut state = ObservationState::default();
            let request = state
                .request(
                    Handle::from_raw(42),
                    ObjectSignals::CLOSED,
                    0,
                    SupervisionPolicy {
                        wait_attempts: 1,
                        ..crate::DEFAULT_SUPERVISION_POLICY
                    },
                )
                .unwrap();
            let at = request.deadline.instant().unwrap().unwrap();
            assert_eq!(
                state.accept(
                    request,
                    ObservationResult::Ready(ObjectSignals::CLOSED),
                    at + delay
                ),
                Ok(true)
            );
        }
    }

    #[test]
    fn identity_deadline_and_single_attempt_are_preserved() {
        let policy = SupervisionPolicy {
            wait_attempts: 1,
            ..crate::DEFAULT_SUPERVISION_POLICY
        };
        let mut state = ObservationState::default();
        let control = Handle::from_raw(42);
        let request = state
            .request(control, ObjectSignals::CLOSED, 0, policy)
            .unwrap();
        assert_eq!(
            state
                .request(control, ObjectSignals::CLOSED, 10, policy)
                .unwrap(),
            request
        );
        let wrong = Observation {
            control: Handle::from_raw(43),
            ..request
        };
        assert!(
            state
                .accept(wrong, ObservationResult::Ready(ObjectSignals::CLOSED), 1)
                .is_err()
        );
        assert!(
            state
                .accept(request, ObservationResult::TimedOut, 1)
                .is_err()
        );
        assert_eq!(
            state.accept(request, ObservationResult::Ready(ObjectSignals::CLOSED), 2),
            Ok(true)
        );
        assert!(
            state
                .accept(request, ObservationResult::Ready(ObjectSignals::CLOSED), 3)
                .is_err()
        );
        let request = state
            .request(control, ObjectSignals::CLOSED, 3, policy)
            .unwrap();
        let at = request.deadline.instant().unwrap().unwrap();
        assert_eq!(
            state.accept(request, ObservationResult::TimedOut, at),
            Ok(false)
        );
        assert_eq!(
            state.request(control, ObjectSignals::CLOSED, at, policy),
            Err(SupervisionCause::Timeout)
        );
    }
}
