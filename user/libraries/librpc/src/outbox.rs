//! 入站请求的有界回复投递与退休。

use erhino_shared::{call::SystemCallError, object::ObjectSignals, time::Deadline};
use libexecution::runtime::{
    Advance, Input, RequestFailure, Requests, SourceEvent, SourceId, SourcePlan, Step,
};

use crate::{
    exchange::{CallCause, PreparedResponse, RequestContext, ResponseFailure, cause},
    outbound::OutboundStage,
};

/// 回复完成后的可观察结果；失败不会隐藏仍由 Outbox 持有的上下文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxResult {
    Sent,
    Abandoned(CallCause),
}

/// 单个入站请求的回复责任。
///
/// Outbox 构造时取得完整回复存储；业务提交前先登记 CLOSED 观察。正文准备好后
/// 尝试发送，只有邮箱满时才将同一来源切换为 WRITABLE|CLOSED 并等待可写。
/// 业务已提交后的回复失败只产生 `Abandoned`，不会伪造业务回滚。
#[derive(Debug)]
pub struct Outbox {
    response: Option<PreparedResponse>,
    deadline: Deadline,
    source_kind: u64,
    source: Option<SourceId>,
    source_writable: bool,
    requested: bool,
    removing: bool,
    rearm_needed: bool,
    operation_retry: bool,
    stage: OutboundStage,
    result: Option<OutboxResult>,
}

impl Outbox {
    #[expect(
        clippy::result_large_err,
        reason = "准备失败原样返还 RequestContext 与 Delivery owner"
    )]
    pub fn prepare(
        context: RequestContext,
        capacity: usize,
        deadline: Deadline,
        source_kind: u64,
    ) -> Result<Self, ResponseFailure> {
        if deadline.instant().is_err() {
            return Err(ResponseFailure {
                error: SystemCallError::IllegalArgument,
                context,
            });
        }
        let response = PreparedResponse::new(context, capacity)?;
        Ok(Self::from_prepared(response, deadline, source_kind))
    }

    pub fn from_prepared(response: PreparedResponse, deadline: Deadline, source_kind: u64) -> Self {
        Self {
            response: Some(response),
            deadline,
            source_kind,
            source: None,
            source_writable: false,
            requested: false,
            removing: false,
            rearm_needed: false,
            operation_retry: false,
            stage: OutboundStage::Ready,
            result: None,
        }
    }

    pub fn response_mut(&mut self) -> Result<&mut PreparedResponse, SystemCallError> {
        self.response.as_mut().ok_or(SystemCallError::ObjectBusy)
    }

    pub fn result(&self) -> Option<OutboxResult> {
        self.result
    }

    pub fn into_context(self) -> Option<RequestContext> {
        self.response.map(PreparedResponse::into_context)
    }

    pub fn drain_capabilities(
        &mut self,
        visit: impl FnMut(rinlib::ipc::capability::Capability, erhino_shared::object::Rights),
    ) {
        if let Some(response) = self.response.as_mut() {
            response.drain_capabilities(visit);
        }
    }

    fn complete(&mut self, result: OutboxResult) {
        if self.result.is_none() {
            self.stage.finish();
            self.result = Some(result);
        }
    }

    fn declare_remove<F>(&mut self, requests: &mut Requests<F>) {
        if self.result.is_some()
            && let Some(source) = self.source
            && !self.removing
        {
            if requests.remove(source).is_ok() {
                self.removing = true;
                self.operation_retry = false;
            } else {
                self.operation_retry = true;
            }
        }
    }

    fn ensure_registration<F>(&mut self, requests: &mut Requests<F>) {
        if self.result.is_some() || self.requested || self.removing {
            return;
        }
        if let Some(source) = self.source {
            // 空邮箱的 WRITABLE 会立即消耗一次性观察；切换前只等 CLOSED。
            if matches!(self.stage, OutboundStage::WaitingWritable) && !self.source_writable {
                if requests.remove(source).is_ok() {
                    self.removing = true;
                    self.operation_retry = false;
                } else {
                    self.operation_retry = true;
                }
                return;
            }
            if self.rearm_needed {
                if requests.rearm(source).is_ok() {
                    self.rearm_needed = false;
                    self.operation_retry = false;
                } else {
                    self.operation_retry = true;
                }
            }
            return;
        }
        if let Some(response) = self.response.as_ref()
            && requests
                .arm_source(
                    SourcePlan::new(
                        response
                            .reply_handle()
                            .expect("live Outbox must retain reply-once owner"),
                        if matches!(self.stage, OutboundStage::WaitingWritable) {
                            ObjectSignals::WRITABLE | ObjectSignals::CLOSED
                        } else {
                            ObjectSignals::CLOSED
                        },
                    ),
                    self.source_kind,
                )
                .is_ok()
        {
            self.requested = true;
            self.source_writable = matches!(self.stage, OutboundStage::WaitingWritable);
            self.rearm_needed = false;
            self.operation_retry = false;
        } else {
            self.operation_retry = true;
        }
    }

    fn attempt_send(&mut self) {
        if self.result.is_some() || !self.stage.can_attempt() || self.source.is_none() {
            return;
        }
        let Some(response) = self.response.as_mut() else {
            return;
        };
        match response.try_send(self.deadline) {
            Ok(()) => {
                self.response = None;
                self.complete(OutboxResult::Sent);
            }
            Err(SystemCallError::MailboxFull) => {
                self.stage.blocked();
                self.rearm_needed = true;
            }
            Err(error) => self.complete(OutboxResult::Abandoned(cause(error))),
        }
    }

    pub fn observe(&mut self, event: SourceEvent) {
        if event.kind != self.source_kind {
            return;
        }
        if event.error != 0 {
            self.complete(OutboxResult::Abandoned(CallCause::System(
                SystemCallError::from_u32(event.error).unwrap_or(SystemCallError::InternalError),
            )));
        } else if event.observed.intersects(ObjectSignals::CLOSED) {
            self.complete(OutboxResult::Abandoned(CallCause::ServiceClosed));
        } else if event.observed.intersects(ObjectSignals::WRITABLE) {
            self.stage.writable();
        }
    }

    pub fn timed_out(&mut self) {
        if self.result.is_none() {
            self.complete(OutboxResult::Abandoned(CallCause::Timeout));
        }
    }

    pub fn is_admitted(&self) -> bool {
        self.source.is_some() && self.result.is_none()
    }

    pub fn is_complete(&self) -> bool {
        self.result.is_some() && self.source.is_none() && !self.requested && !self.removing
    }

    /// 业务 Commit 前只完成来源准入，不发送尚未编码的回复。
    pub fn admit<F>(&mut self, requests: &mut Requests<F>) -> Advance {
        self.operation_retry = false;
        self.ensure_registration(requests);
        Advance {
            work_done: 1,
            step: if self.is_admitted() || self.operation_retry || self.requested {
                Step::Runnable
            } else {
                Step::Parked
            },
        }
    }

    pub fn drive<F>(
        &mut self,
        requests: &mut Requests<F>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        self.operation_retry = false;
        for _ in 0..budget {
            if self.result.is_none() {
                self.ensure_registration(requests);
                self.attempt_send();
            }
            self.declare_remove(requests);
        }
        Ok(Advance {
            work_done: budget,
            step: if self.is_complete() {
                Step::Complete
            } else if self.operation_retry
                || self.result.is_some()
                || self.rearm_needed
                || self.requested
                || self.removing
            {
                Step::Runnable
            } else {
                Step::Parked
            },
        })
    }

    pub fn advance<F>(
        &mut self,
        requests: &mut Requests<F>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.observe(event);
        }
        if input.take_timeout() {
            self.timed_out();
        }
        self.drive(requests, budget)
    }

    pub fn registered<W>(&mut self, _world: &mut W, kind: u64, source: SourceId) {
        if kind == self.source_kind {
            self.requested = false;
            self.source = Some(source);
        }
    }

    pub fn unregistered<W>(&mut self, _world: &mut W, kind: u64, source: SourceId) {
        if kind == self.source_kind && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.source_writable = false;
            self.removing = false;
        }
    }

    pub fn refused<W, F>(&mut self, _world: &mut W, failure: RequestFailure<F>) {
        match failure {
            RequestFailure::Source { kind, error } if kind == self.source_kind => {
                self.requested = false;
                self.complete(OutboxResult::Abandoned(cause(error)));
            }
            RequestFailure::Wake { error, .. } => {
                self.complete(OutboxResult::Abandoned(cause(error)));
            }
            RequestFailure::Spawn { .. } => {}
            RequestFailure::Source { .. } => {}
        }
    }

    pub fn stop<W>(&mut self, _world: &mut W) {
        self.complete(OutboxResult::Abandoned(CallCause::Shutdown));
    }

    pub fn deadline(&self) -> Deadline {
        if self.result.is_some() {
            Deadline::INFINITE
        } else {
            self.deadline
        }
    }
}
