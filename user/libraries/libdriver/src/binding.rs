//! 逐激活的唯一首次核验、Binding 观察与受限静止报告；由宿主已有 Runtime 调度。
use crate::{protocol::Phase, worker::ActivationSlot};
use erhino_shared::time::Deadline;
use libdevice::{
    Error,
    binding::{Phase as BindingPhase, UseAccess},
    protocol::{self, Command, UseInfo},
};
use libexecution::runtime::{Advance, Input, RequestFailure, Requests, SourceId, Step};
use librpc::{
    dispatcher::{Dispatcher, ServiceOwner},
    subscription::ConsumerTask,
};

pub struct BindingTask {
    call: Option<(u64, Command<'static>)>,
    info: Option<UseInfo>,
    watch: Option<ConsumerTask<libdevice::client::Watch>>,
    snapshot: Option<libdevice::binding::Snapshot>,
    reported: bool,
    closing: bool,
    end: Deadline,
}
impl Default for BindingTask {
    fn default() -> Self {
        Self::new()
    }
}
impl BindingTask {
    pub fn new() -> Self {
        Self {
            call: None,
            info: None,
            watch: None,
            snapshot: None,
            reported: false,
            closing: false,
            end: Deadline::INFINITE,
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if let Some(watch) = self.watch.as_mut() {
            watch.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        if let Some(watch) = self.watch.as_mut() {
            watch.unregistered(id);
        }
    }
    pub fn deadline(&self) -> Deadline {
        self.end
    }
    pub fn refused<F>(&mut self, failure: RequestFailure<F>) -> Result<(), Error> {
        // Watch 的来源准入失败属于原核验/观察 owner，不能绕过准备拒绝和退休。
        if let Some(watch) = self.watch.as_mut() {
            watch.refused(failure);
            Ok(())
        } else {
            Err(Error::Unavailable)
        }
    }
    // 三个任务身份分别用于自身完成、硬件唤醒和运输推进，不另包装宿主上下文。
    #[allow(clippy::too_many_arguments)]
    pub fn advance<F>(
        &mut self,
        slot: &mut ActivationSlot,
        task: u64,
        engine: u64,
        transport: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Result<Advance, Error> {
        slot.attach_lifecycle(task);
        let next = match self.drive(slot, task, engine, transport, rpc, requests, input) {
            Ok(next) => next,
            Err(error) if !slot.io_ready() => {
                slot.reject_preparing(error)?;
                self.closing = true;
                requests.wake(engine)?;
                Step::Runnable
            }
            Err(error) => return Err(error),
        };
        if next == Step::Complete {
            slot.retire_lifecycle();
            requests.wake(engine)?;
        }
        Ok(Advance {
            work_done: 1,
            step: next,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn drive<F>(
        &mut self,
        slot: &mut ActivationSlot,
        task: u64,
        engine: u64,
        transport: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Result<Step, Error> {
        if !slot.active() {
            self.closing = true;
        }
        if self.closing {
            return self.retire(task, transport, rpc, requests, input);
        }
        if slot.activation.is_none() {
            return Ok(Step::Parked);
        }
        if self.end == Deadline::INFINITE && self.info.is_none() {
            self.end = libkernel::time::timeout_millis(5_000)?;
        }
        if let Some((txid, command)) = self.call {
            requests.wake(transport)?;
            if let Some(result) = rpc.take(txid) {
                self.call = None;
                let result = result
                    .result
                    .map_err(|error| {
                        libkernel::debug!(
                            "driver binding transport failed: txid={}, command={:?}, phase={:?}, cause={:?}",
                            txid, command, error.phase, error.cause
                        );
                        Error::Unavailable
                    })
                    .and_then(|reply| {
                        let (header, body) = protocol::Header::decode(&reply.payload)?;
                        if header.op != command.op() {
                            return Err(Error::Invalid);
                        }
                        if let Some(error) = header.error {
                            if matches!(command, Command::ReportQuiescent)
                                && error == Error::Closed
                                && body.is_empty()
                                && reply.handles.remaining() == 0
                            {
                                // 中央可能先凭未暴露证明释放；仍等原 Watch 的 Released 快照。
                                self.reported = true;
                                return Ok(());
                            }
                            return Err(error);
                        }
                        match command {
                            Command::QueryUse => {
                                if reply.handles.remaining() != 0 {
                                    return Err(Error::Invalid);
                                }
                                let info = UseInfo::decode(body)?;
                                if info.binding.reference != slot.reference()
                                    || info.binding.phase != BindingPhase::ResourcesReady
                                {
                                    return Err(Error::Closed);
                                }
                                if info.stop_requirement
                                    == libdevice::binding::StopRequirement::ConsumerQuiescence
                                    && !info.access.contains(UseAccess::REPORT_QUIESCENT)
                                {
                                    return Err(Error::Permission);
                                }
                                self.info = Some(info);
                            }
                            Command::WatchBinding => {
                                let registry = slot
                                    .activation
                                    .as_ref()
                                    .ok_or(Error::Closed)?
                                    .usage()?
                                    .description()?
                                    .related_object_id;
                                let watch = libdevice::client::Watch::from_reply(
                                    reply,
                                    registry,
                                    command.op(),
                                )?;
                                self.snapshot = watch.info().binding;
                                if self
                                    .snapshot
                                    .is_none_or(|snapshot| snapshot.reference != slot.reference())
                                {
                                    return Err(Error::Invalid);
                                }
                                self.watch = Some(ConsumerTask::new(watch, Deadline::INFINITE));
                            }
                            Command::ReportQuiescent => {
                                if !body.is_empty() || reply.handles.remaining() != 0 {
                                    return Err(Error::Invalid);
                                }
                                self.reported = true;
                            }
                            _ => return Err(Error::Invalid),
                        }
                        Ok(())
                    });
                result?;
            }
        }
        if let Some(watch) = self.watch.as_mut() {
            let advance = watch.advance(task, transport, rpc, requests, input, 1);
            if watch.failure().is_some() {
                return Err(Error::Unavailable);
            }
            let available = watch.snapshot();
            if let Some(info) = available {
                let snapshot = info.binding.ok_or(Error::Invalid)?;
                if snapshot.reference != slot.reference() {
                    return Err(Error::StaleEpoch);
                }
                self.snapshot = Some(snapshot);
                watch.acknowledge_read()?;
                requests.wake(engine)?;
            }
            if let Some(snapshot) = self.snapshot {
                match snapshot.phase {
                    BindingPhase::ResourcesReady if watch.armed() && !slot.io_ready() => {
                        slot.allow_io()?;
                        self.end = Deadline::INFINITE;
                        requests.wake(engine)?;
                    }
                    BindingPhase::Stopping | BindingPhase::Released => {
                        let changed = matches!(
                            slot.activation().ok_or(Error::NotReady)?.snapshot().phase,
                            Phase::Starting | Phase::Ready
                        );
                        slot.stop()?;
                        if !slot.io_ready() {
                            return Err(Error::Closed);
                        }
                        if self.end == Deadline::INFINITE {
                            self.end = libkernel::time::timeout_millis(20_000)?;
                        }
                        if snapshot.phase == BindingPhase::Released
                            && slot.activation().ok_or(Error::NotReady)?.snapshot().phase
                                == Phase::Terminal
                        {
                            slot.release(snapshot)?;
                            self.closing = true;
                            requests.wake(engine)?;
                            return Ok(Step::Runnable);
                        }
                        if changed {
                            requests.wake(engine)?;
                        }
                    }
                    _ => {}
                }
            }
            if self.call.is_none()
                && self
                    .snapshot
                    .is_some_and(|snapshot| snapshot.phase == BindingPhase::Stopping)
                && slot
                    .activation
                    .as_ref()
                    .is_some_and(|activation| activation.snapshot().consumer_stopped)
                && self
                    .info
                    .is_some_and(|info| info.access.contains(UseAccess::REPORT_QUIESCENT))
                && !self.reported
            {
                // consumer_stopped 只能由实际 Engine 的 unsafe 证据提交点置位。
                self.begin(
                    slot,
                    task,
                    transport,
                    rpc,
                    requests,
                    Command::ReportQuiescent,
                )?;
                return Ok(Step::Runnable);
            }
            if self
                .end
                .instant()
                .map_err(|_| Error::Invalid)?
                .is_some_and(|end| input.now_ns() >= end)
            {
                return Err(Error::Unavailable);
            }
            return Ok(if available.is_some() {
                // acknowledge_read 已提交下一步 Ack，不能沿调用前的 Reading 停驻。
                Step::Runnable
            } else if advance.step == Step::Complete {
                Step::Parked
            } else {
                advance.step
            });
        }
        if self.call.is_some() {
            return Ok(Step::Parked);
        }
        let command = if self.info.is_none() {
            Command::QueryUse
        } else {
            Command::WatchBinding
        };
        self.begin(slot, task, transport, rpc, requests, command)?;
        Ok(Step::Runnable)
    }
    fn retire<F>(
        &mut self,
        task: u64,
        transport: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Result<Step, Error> {
        self.end = Deadline::INFINITE;
        if let Some((txid, _)) = self.call {
            rpc.cancel(txid);
            requests.wake(transport)?;
            let Some(completion) = rpc.take(txid) else {
                return Ok(Step::Parked);
            };
            // 拒绝准备或真实 Released 已封业务提交；迟到回复/主动取消只退运输。
            drop(completion);
            self.call = None;
        }
        if let Some(watch) = self.watch.as_mut() {
            watch.stop();
            return Ok(watch.advance(task, transport, rpc, requests, input, 1).step);
        }
        Ok(Step::Complete)
    }
    fn begin<F>(
        &mut self,
        slot: &ActivationSlot,
        task: u64,
        transport: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        command: Command<'static>,
    ) -> Result<(), Error> {
        let usage = slot
            .activation
            .as_ref()
            .ok_or(Error::Closed)?
            .usage()?
            .clone();
        let mut bytes = [0; protocol::HEADER_LEN];
        let used = command.encode(self.end, &mut bytes)?;
        let request = librpc::Request::new(protocol::ID, &bytes[..used])?;
        let txid = rpc
            .begin_for(
                ServiceOwner::Shared(usage),
                self.end,
                request,
                Some(task),
                requests.task_funding(),
            )
            .map_err(|_| Error::Unavailable)?;
        self.call = Some((txid, command));
        requests.wake(transport)?;
        Ok(())
    }
}
