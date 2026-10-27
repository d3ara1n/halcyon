//! 原 Claim 任务中的长期 RPC 责任；宿主使用既有 Runtime/Dispatcher 推进。
//! 不因回复丢失、最后控制 Close 或本地取消而把可能存在的供应效果记成 Released。
use crate::{
    Error,
    binding::{Commit, Phase, Snapshot},
    funding::Funding,
    identity::BindingRef,
    protocol::{self, Progress, ProvisionCommand},
    registry::Registry,
    resource::{Collecting, DeviceResource, Entry, Frozen},
};
use alloc::{rc::Rc, sync::Arc};
use core::cell::RefCell;
use erhino_shared::{object::Rights, time::Deadline};
use libbudget::Charge;
use libexecution::runtime::{Advance, Input, Requests, Step};
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use librpc::{
    Request,
    dispatcher::{Dispatcher, ServiceOwner},
    exchange::{CallError, CallOwner},
};

pub type Backing = Registry<Arc<MailboxSender>>;

/// 部署 owner 提供实际期限和重查间隔；这些参数不作为硬件完成证据。
#[derive(Clone, Copy)]
pub struct Policy {
    pub prepare_ns: u64,
    pub stop_ns: u64,
    pub retry_ns: u64,
}
impl Policy {
    fn validate(self) -> Result<(), Error> {
        if self.prepare_ns == 0
            || self.stop_ns == 0
            || self.retry_ns == 0
            || self.retry_ns > self.prepare_ns
            || self.retry_ns > self.stop_ns
        {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
}
struct Shared {
    reference: BindingRef,
    source: u64,
    task: u64,
    resources: Option<Frozen<Capability>>,
    released: Option<Snapshot>,
    reported: bool,
    can_report: bool,
    dma_report: bool,
    // 唯一资源出口可能交出能力的不可逆事实；关闭别名/回复丢失不能清除。
    resources_exposed: bool,
    fault: Option<Error>,
    _unit: Charge,
    _bytes: Charge,
}

/// 服务端控制表及已接请求持有此共享观察；不是跨进程 capability。
/// 只有真实 sender context 通过角色表验证后才能调用控制方法。
#[derive(Clone)]
pub struct Binding(Rc<RefCell<Shared>>);
impl Binding {
    pub fn snapshot(&self, registry: &Backing) -> Result<Snapshot, Error> {
        let core = self.0.borrow();
        if let Some(released) = core.released {
            return Ok(released);
        }
        registry.binding_snapshot(core.reference)
    }
    pub fn fault(&self) -> Option<Error> {
        self.0.borrow().fault
    }
    pub fn task(&self) -> u64 {
        self.0.borrow().task
    }
    /// 宿主在同次事件处理中唤醒返回的原任务；重复 Stop 不创建新任务。
    pub fn stop(&self, registry: &mut Backing) -> Result<u64, Error> {
        let core = self.0.borrow();
        if core.released.is_none() {
            registry.stop_claim(core.source, core.reference)?;
        }
        Ok(core.task)
    }
    pub fn report_stopped(&self) -> Result<u64, Error> {
        let mut core = self.0.borrow_mut();
        if !core.can_report || !core.dma_report {
            return Err(Error::Permission);
        }
        core.reported = true;
        Ok(core.task)
    }
    /// 只投影同一Frozen的描述，不复制能力，不改变resources_exposed。
    pub fn discover_resources(
        &self,
        registry: &Backing,
        wire: &[u8],
        handle_count: usize,
        out: &mut [u8],
    ) -> Result<usize, liboal::protocol::Status> {
        let core = self.0.borrow();
        if core.released.is_some() {
            return Err(liboal::protocol::Status::Revoked);
        }
        registry
            .binding_usable(core.reference)
            .map_err(crate::registry::oal::status)?;
        core.resources
            .as_ref()
            .ok_or(liboal::protocol::Status::Busy)?
            .discover(core.reference, wire, handle_count, out)
    }
    /// 只返回固定导出 rights 的描述；运输中的临时 alias 额外保留 TRANSIT，
    /// 宿主 push 时必须用描述中的 rights，不能把临时运输权一并授予消费者。
    pub fn resource(
        &self,
        registry: &Backing,
        name: &str,
        out: &mut [u8],
    ) -> Result<(usize, Capability, Rights), Error> {
        let mut core = self.0.borrow_mut();
        if core.released.is_some() {
            return Err(Error::Closed);
        }
        registry.binding_usable(core.reference)?;
        let table = core.resources.as_ref().ok_or(Error::NotReady)?;
        let item = table.get(name).ok_or(Error::NotFound)?;
        let entry = Entry {
            index: 0,
            descriptor: item.descriptor,
            name: item.name,
            metadata: item.metadata,
        };
        let used = entry.encode(protocol::HEADER_LEN, out)?;
        let alias = item
            .capability
            .duplicate(item.descriptor.rights | Rights::TRANSIT)?;
        let rights = item.descriptor.rights;
        // 同一 actor 内，在可用性门和复制成功之后、能力离开 owner 进入 Outbox 之前提交。
        let first = !core.resources_exposed;
        core.resources_exposed = true;
        if first {
            libkernel::debug!(
                "device resources exposed: reference={:?}, name={}, resources_exposed={}",
                core.reference,
                name,
                core.resources_exposed
            );
        }
        Ok((used, alias, rights))
    }
}

#[derive(Clone, Copy, Debug)]
enum Next {
    Prepare,
    Get(u32),
    Ready,
    Stop,
    Confirm,
    Release,
    Released,
}

pub struct Work {
    binding: Binding,
    collecting: Option<Collecting<Capability>>,
    provider: Arc<MailboxSender>,
    funding: Funding,
    policy: Policy,
    next: Next,
    call: Option<(u64, ProvisionCommand)>,
    cleanup: Option<CallError>,
    prepared_count: u32,
    prepare_end: u64,
    stop_end: Option<u64>,
    retry_at: Option<u64>,
    wake_transport: bool,
    confirmed: bool,
    stop_acknowledged: bool,
    committed: bool,
    retired: bool,
    _storage: Charge,
}
impl core::fmt::Debug for Work {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let shared = self.binding.0.borrow();
        f.debug_struct("Work")
            .field("reference", &shared.reference)
            .field("task", &shared.task)
            .field("next", &self.next)
            .field("call", &self.call)
            .field("stop_end", &self.stop_end)
            .field("retry_at", &self.retry_at)
            .field("confirmed", &self.confirmed)
            .field("stop_acknowledged", &self.stop_acknowledged)
            .field("reported", &shared.reported)
            .field("resources_exposed", &shared.resources_exposed)
            .field("fault", &shared.fault)
            .finish()
    }
}
impl Work {
    /// 所有表存储、任务本体和 Binding 记录费用均在 Claim 提交前准备。
    /// 宿主还须把包含本 Work 的 TaskSubmission 成功准入到实际 Runtime。
    pub fn prepare(
        registry: &Backing,
        source: u64,
        reference: BindingRef,
        policy: Policy,
    ) -> Result<Self, Error> {
        policy.validate()?;
        registry.claim(source, reference)?;
        let funding = registry.funding(source)?.clone();
        let collecting = Collecting::reserve(
            registry.limits(reference.device())?,
            protocol::RESOURCE_HEADER_LEN,
            funding.domain(),
        )?;
        let storage = funding
            .domain()
            .acquire(DeviceResource::Bytes, core::mem::size_of::<Self>())?;
        let unit = funding.domain().acquire(DeviceResource::Binding, 1)?;
        let bytes = funding
            .domain()
            .acquire(DeviceResource::Bytes, core::mem::size_of::<Shared>())?;
        let shared = Rc::try_new(RefCell::new(Shared {
            reference,
            source,
            task: 0,
            resources: None,
            released: None,
            reported: false,
            can_report: registry.report_authorized(source, reference).is_ok(),
            dma_report: false,
            resources_exposed: false,
            fault: None,
            _unit: unit,
            _bytes: bytes,
        }))
        .map_err(|_| Error::Resource)?;
        Ok(Self {
            binding: Binding(shared),
            collecting: Some(collecting),
            provider: registry.provision(reference.device())?.clone(),
            funding,
            policy,
            next: Next::Prepare,
            call: None,
            cleanup: None,
            prepared_count: 0,
            prepare_end: 0,
            stop_end: None,
            retry_at: None,
            wake_transport: false,
            confirmed: false,
            stop_acknowledged: false,
            committed: false,
            retired: false,
            _storage: storage,
        })
    }
    pub fn funding(&self) -> &Funding {
        &self.funding
    }
    /// 仅放入尚未链接的控制表候选；commit 返回 Created 前不得发布对应 sender。
    pub fn prepared_binding(&self) -> Binding {
        self.binding.clone()
    }
    /// 仅实际 Runtime 任务 advance 中调用；Existing 的候选不得开始 Prepare。
    pub fn commit(&mut self, task: u64, now: u64, registry: &mut Backing) -> Result<Commit, Error> {
        if self.committed || task == 0 {
            return Err(Error::Invalid);
        }
        let end = now
            .checked_add(self.policy.prepare_ns)
            .ok_or(Error::Exhausted)?;
        let core = self.binding.0.borrow();
        let source = core.source;
        let reference = core.reference;
        drop(core);
        registry.retain_source(source)?;
        if let Err(error) = registry.retain_device(reference.device()) {
            registry.release_source(source)?;
            return Err(error);
        }
        let result = registry.commit_claim(source, reference, task);
        if matches!(result, Ok(Commit::Created(_))) {
            self.binding.0.borrow_mut().task = task;
            self.prepare_end = end;
            self.committed = true;
        } else {
            registry.release_device(reference.device())?;
            registry.release_source(source)?;
        }
        result
    }
    /// Created 提交后安装进控制表；最后一个角色/在途请求退休时应丢该别名并唤醒原任务。
    pub fn binding(&self) -> Result<Binding, Error> {
        if !self.committed || self.retired {
            return Err(Error::NotReady);
        }
        Ok(self.binding.clone())
    }
    pub fn deadline(&self) -> Deadline {
        // 在途调用的期限由 Dispatcher 持有；空闲重查仍必须独立保有阶段期限。
        let phase_end = if !self.committed
            || self.retired
            || self.has_fault()
            || self.call.is_some()
            || matches!(self.next, Next::Ready | Next::Released)
        {
            None
        } else {
            Some(self.stop_end.unwrap_or(self.prepare_end))
        };
        crate::timing::deadline(self.retry_at, phase_end).map_or(Deadline::INFINITE, Deadline::at)
    }
    pub fn has_fault(&self) -> bool {
        self.binding.0.borrow().fault.is_some()
    }
    /// Runtime 已接受 Wake 意图但 transport 任务无法被唤醒时，宿主转交这一失败。
    pub fn transport_wake_failed(&mut self) {
        self.fault(Error::Unavailable);
    }
    fn fault(&mut self, error: Error) {
        self.binding.0.borrow_mut().fault.get_or_insert(error);
        self.retry_at = None;
    }
    fn schedule(&mut self, now: u64) {
        self.retry_at = now.checked_add(self.policy.retry_ns);
        if self.retry_at.is_none() {
            self.fault(Error::Exhausted);
        }
    }
    fn stop(&mut self, now: u64, registry: &mut Backing) -> Result<(), Error> {
        let core = self.binding.0.borrow();
        registry.stop_claim(core.source, core.reference)?;
        drop(core);
        if self.stop_end.is_none() {
            let core = self.binding.0.borrow();
            libkernel::debug!(
                "device Binding stopping: reference={:?}, resources_exposed={}, reported={}",
                core.reference,
                core.resources_exposed,
                core.reported
            );
            drop(core);
            self.stop_end = Some(
                now.checked_add(self.policy.stop_ns)
                    .ok_or(Error::Exhausted)?,
            );
        }
        if !matches!(self.next, Next::Release | Next::Released) {
            self.next = Next::Stop;
            self.retry_at = None;
        }
        Ok(())
    }
    fn closed(&mut self, now: u64, registry: &mut Backing) -> Result<(), Error> {
        self.stop(now, registry)?;
        // 调用结果只在 Dispatcher 已退休原调用来源后取走；不存在本任务的另一条在途调用。
        self.next = Next::Release;
        self.retry_at = None;
        self.finish_release(registry)
    }
    fn finish_release(&mut self, registry: &mut Backing) -> Result<(), Error> {
        let reference = self.binding.0.borrow().reference;
        let released = match registry.release_binding(reference) {
            Ok(released) => released,
            Err(Error::Busy) => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut core = self.binding.0.borrow_mut();
        core.released = Some(released);
        core.resources = None;
        drop(core);
        self.collecting = None;
        self.next = Next::Released;
        self.retry_at = None;
        Ok(())
    }
    fn consume(
        &mut self,
        command: ProvisionCommand,
        mut reply: librpc::exchange::Reply,
        now: u64,
        registry: &mut Backing,
    ) -> Result<(), Error> {
        let reference = self.binding.0.borrow().reference;
        let stopping = registry.binding_snapshot(reference)?.phase == Phase::Stopping;
        match command {
            ProvisionCommand::Get { epoch, index } => {
                if stopping {
                    return self.stop(now, registry);
                }
                let entry = protocol::decode_resource(
                    &reply.payload,
                    epoch,
                    index,
                    reply.handles.remaining(),
                )?;
                let capability = reply.handles.take(0).map_err(|_| Error::Invalid)?;
                entry.descriptor.check_handle(&capability.description()?)?;
                self.collecting
                    .as_mut()
                    .ok_or(Error::NotReady)?
                    .push(&entry, capability)?;
                let next = index.checked_add(1).ok_or(Error::Exhausted)?;
                if next == self.prepared_count {
                    self.publish(registry)?;
                } else {
                    self.next = Next::Get(next);
                }
            }
            _ => {
                let progress =
                    Progress::decode(&reply.payload, command, reply.handles.remaining())?;
                if matches!(command, ProvisionCommand::Stop(_)) {
                    self.stop_acknowledged = true;
                }
                // Closed 是原 owner 的明确安全承诺；领域错误 Closed 不走此分支。
                match progress {
                    Progress::Closed | Progress::Rejected(_) => self.closed(now, registry)?,
                    Progress::Failed(_) => {
                        if matches!(command, ProvisionCommand::Prepare(_)) {
                            self.stop(now, registry)?;
                        } else {
                            self.schedule(now);
                            self.next = Next::Stop;
                        }
                    }
                    Progress::Prepared(count)
                        if matches!(command, ProvisionCommand::Prepare(_)) && !stopping =>
                    {
                        self.collecting
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepared(count)?;
                        self.prepared_count = count;
                        if count == 0 {
                            self.publish(registry)?;
                        } else {
                            self.next = Next::Get(0);
                        }
                    }
                    Progress::Preparing
                        if matches!(command, ProvisionCommand::Prepare(_)) && !stopping =>
                    {
                        self.next = Next::Prepare;
                        self.schedule(now);
                    }
                    Progress::Stopping | Progress::Prepared(_) | Progress::Preparing => {
                        self.stop(now, registry)?;
                        if matches!(command, ProvisionCommand::ConfirmStopped(_)) {
                            self.confirmed = true;
                        }
                        self.schedule(now);
                    }
                }
            }
        }
        Ok(())
    }
    fn publish(&mut self, registry: &mut Backing) -> Result<(), Error> {
        let collecting = self.collecting.take().ok_or(Error::NotReady)?;
        let frozen = match collecting.finish() {
            Ok(frozen) => frozen,
            Err(collecting) => {
                self.collecting = Some(collecting);
                return Err(Error::NotReady);
            }
        };
        let reference = self.binding.0.borrow().reference;
        // 即便准入在收表期间关闭，也保留整表别名直至原供应实际 Closed。
        let mut core = self.binding.0.borrow_mut();
        core.dma_report = (0..frozen.len()).any(|index| {
            frozen.at(index).is_some_and(|item| {
                matches!(
                    item.descriptor.kind,
                    erhino_shared::object::ObjectKind::DmaUse
                        | erhino_shared::object::ObjectKind::DmaDomainControl
                )
            })
        });
        core.resources = Some(frozen);
        drop(core);
        registry.resources_ready(reference)?;
        self.next = Next::Ready;
        Ok(())
    }
    fn transport_error(
        &mut self,
        mut error: CallError,
        txid: u64,
        command: ProvisionCommand,
        now: u64,
        registry: &mut Backing,
    ) {
        libkernel::debug!(
            "device Provision RPC failed: reference={:?}, command={:?}, txid={}, phase={:?}, cause={:?}",
            self.binding.0.borrow().reference,
            command,
            txid,
            error.phase,
            error.cause
        );
        if matches!(error.owner, CallOwner::Unsent(_)) {
            drop(error.take_unsent_request());
        }
        if error.has_unretired_owners() {
            self.cleanup = Some(error);
        }
        if matches!(
            command,
            ProvisionCommand::Stop(_) | ProvisionCommand::ConfirmStopped(_)
        ) {
            self.fault(Error::Unavailable);
        } else if let Err(error) = self.stop(now, registry) {
            self.fault(error);
        }
    }
    /// 一次只推进一项业务动作；运输/来源注销仍由原 Dispatcher/Runtime 拥有。
    /// 已提交后的失败以 fault 保留责任，宿主必须升级管理故障，不能丢弃本 Work 当回滚。
    pub fn advance<F>(
        &mut self,
        registry: &mut Backing,
        dispatcher: &mut Dispatcher,
        transport_task: u64,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Advance {
        let now = input.now_ns();
        while input.pull().is_some() {}
        input.take_timeout();
        if !self.committed || self.retired {
            return step(Step::Complete);
        }
        if self.wake_transport {
            if requests.wake(transport_task).is_err() {
                return step(Step::Runnable);
            }
            self.wake_transport = false;
        }
        if let Some(error) = self.cleanup.as_mut() {
            if error.retry_cleanup().is_err() {
                self.schedule(now);
                return step(Step::Parked);
            }
            self.cleanup = None;
        }
        if let Some((txid, command)) = self.call {
            if let Some(completion) = dispatcher.take(txid) {
                self.call = None;
                match completion.result {
                    Ok(reply) => {
                        if let Err(error) = self.consume(command, reply, now, registry) {
                            if matches!(
                                command,
                                ProvisionCommand::Stop(_) | ProvisionCommand::ConfirmStopped(_)
                            ) {
                                self.fault(error);
                            } else if let Err(error) = self.stop(now, registry) {
                                self.fault(error);
                            }
                        }
                    }
                    Err(error) => self.transport_error(error, txid, command, now, registry),
                }
                return step(Step::Runnable);
            }
            let reference = self.binding.0.borrow().reference;
            if registry
                .binding_snapshot(reference)
                .is_ok_and(|s| s.phase == Phase::Stopping)
                && !matches!(
                    command,
                    ProvisionCommand::Stop(_) | ProvisionCommand::ConfirmStopped(_)
                )
                && dispatcher.cancel(txid)
            {
                self.wake_transport = true;
                return step(Step::Runnable);
            }
            return step(Step::Parked);
        }
        if self.has_fault() {
            return step(Step::Parked);
        }
        if matches!(self.next, Next::Released) {
            if Rc::strong_count(&self.binding.0) > 1 {
                return step(Step::Parked);
            }
            let core = self.binding.0.borrow();
            let result = registry
                .release_device(core.reference.device())
                .and_then(|()| registry.release_source(core.source));
            drop(core);
            if let Err(error) = result {
                self.fault(error);
                return step(Step::Parked);
            }
            self.retired = true;
            return step(Step::Complete);
        }
        let reference = self.binding.0.borrow().reference;
        match registry.binding_snapshot(reference) {
            Ok(snapshot)
                if snapshot.phase == Phase::Stopping
                    && !matches!(self.next, Next::Stop | Next::Confirm | Next::Release) =>
            {
                if let Err(error) = self.stop(now, registry) {
                    self.fault(error);
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.fault(error);
                return step(Step::Parked);
            }
        }
        if matches!(self.next, Next::Ready) {
            return step(Step::Parked);
        }
        if matches!(self.next, Next::Release) {
            if let Err(error) = self.finish_release(registry) {
                self.fault(error);
            } else if matches!(self.next, Next::Released) {
                return step(Step::Runnable);
            } else if self.stop_end.is_some_and(|end| now >= end) {
                self.fault(Error::Unavailable);
            }
            // 自身供应安全事实已保存；只等待原维护在依赖变化时唤醒，不再调用供应者。
            return step(Step::Parked);
        }
        let consumer_finished = {
            let core = self.binding.0.borrow();
            // 内生证明不借用外部硬件报告权限，也不依赖收表完成后的 dma_report。
            // Stop 已提交后 Get 门永久关闭；未曾交付资源则迟到激活不能产生访问。
            core.reported
                || (!core.resources_exposed
                    && registry
                        .binding_snapshot(core.reference)
                        .is_ok_and(|snapshot| snapshot.phase == Phase::Stopping))
        };
        let report_ready = matches!(self.next, Next::Stop)
            && self.stop_acknowledged
            && consumer_finished
            && !self.confirmed;
        match crate::timing::gate(
            now,
            self.retry_at,
            Some(self.stop_end.unwrap_or(self.prepare_end)),
            report_ready,
        ) {
            crate::timing::Gate::Waiting => return step(Step::Parked),
            crate::timing::Gate::Expired if self.stop_end.is_some() => {
                self.fault(Error::Unavailable);
                return step(Step::Parked);
            }
            crate::timing::Gate::Expired => {
                if let Err(error) = self.stop(now, registry) {
                    self.fault(error);
                    return step(Step::Parked);
                }
            }
            crate::timing::Gate::Due => {}
        }
        self.retry_at = None;
        if report_ready {
            self.next = Next::Confirm;
        }
        let command = match self.next {
            Next::Prepare => ProvisionCommand::Prepare(reference.epoch()),
            Next::Get(index) => ProvisionCommand::Get {
                epoch: reference.epoch(),
                index,
            },
            Next::Stop => ProvisionCommand::Stop(reference.epoch()),
            Next::Confirm => ProvisionCommand::ConfirmStopped(reference.epoch()),
            Next::Ready | Next::Release | Next::Released => return step(Step::Parked),
        };
        let deadline = Deadline::at(self.stop_end.unwrap_or(self.prepare_end));
        let mut bytes = [0; protocol::HEADER_LEN + 12];
        let request = command.encode(deadline, &mut bytes).and_then(|used| {
            Request::new(protocol::PROVISION_ID, &bytes[..used]).map_err(Error::from)
        });
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.fault(error);
                return step(Step::Parked);
            }
        };
        let task = self.binding.0.borrow().task;
        match dispatcher.begin_for(
            ServiceOwner::Shared(self.provider.clone()),
            deadline,
            request,
            Some(task),
            self.funding.execution(),
        ) {
            Ok(txid) => {
                self.call = Some((txid, command));
                self.wake_transport = true;
                step(Step::Runnable)
            }
            Err(failure) => {
                self.transport_error(failure.error, 0, command, now, registry);
                step(Step::Runnable)
            }
        }
    }
}
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
