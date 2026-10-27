//! root显式组合原RPC调用owner；整个等待只使用根Assembly。

use super::*;
use librpc::{CallCause, CallError, CallOperation, Exchange, Reply, Request};

impl RootSupervisor {
    /// 等真实回复就绪但不领取，用于原队列持有验收；其它职责照常推进。
    pub(crate) fn wait_rpc_ready(
        &mut self,
        operation: &CallOperation<'_>,
        deadline: Deadline,
    ) -> Result<(), CallCause> {
        loop {
            self.turn_and_wait(Some(Await::Turn))
                .map_err(CallCause::System)?;
            if operation.poll_ready()? {
                return Ok(());
            }
            let sources = operation.wait_sources();
            let count = sources
                .iter()
                .take_while(|(handle, _)| handle.is_valid())
                .count();
            self.wait_sources(&sources[..count], deadline)
                .map_err(CallCause::System)?;
        }
    }
}

impl Exchange for RootSupervisor {
    fn call_cancelable(
        &mut self,
        service: &MailboxSender,
        deadline: Deadline,
        request: Request,
        cancel: Option<Handle>,
    ) -> Result<Reply, CallError> {
        // 任务不能回调root装配；唯一顺序调用暂时借出原Caller，普通成功后复用ReplyPort。
        let mut caller = self
            .caller
            .take()
            .expect("root RPC cannot reenter its sequential owner");
        let result = (|| {
            let mut operation =
                CallOperation::new_composed(&mut caller, service, deadline, request, cancel)?;
            let result = (|| loop {
                self.turn_and_wait(Some(Await::Turn))
                    .map_err(CallCause::System)?;
                if let Some(reply) = operation.advance()? {
                    return Ok(reply);
                }
                if operation.poll_ready()? {
                    continue;
                }
                let sources = operation.wait_sources();
                let count = sources
                    .iter()
                    .take_while(|(handle, _)| handle.is_valid())
                    .count();
                self.wait_sources(&sources[..count], deadline)
                    .map_err(CallCause::System)?;
            })();
            // 所有错误都保留未完成 operation；wait_sources 返还前已实际注销来源。
            // 统一 abort 根据原投递阶段返还请求或隔离 ReplyPort，调用能力始终保活。
            result.map_err(|cause| {
                operation
                    .abort(cause)
                    .expect("pending root call remains owned")
            })
        })();
        self.caller = Some(caller);
        result
    }
}
