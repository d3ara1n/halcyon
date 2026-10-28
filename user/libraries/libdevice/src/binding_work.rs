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
use alloc::{boxed::Box, rc::Rc, sync::Arc};
use core::cell::RefCell;
use erhino_shared::{
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::Charge;
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, Requests, SourceId, SourcePlan, Step},
};
use libkernel::ipc::{
    capability::Capability,
    message::MailboxSender,
    notification::{self, NotificationPair},
};
use librpc::{
    Request,
    dispatcher::{Dispatcher, ServiceOwner},
    exchange::CallError,
};

pub type Backing = Registry<Arc<MailboxSender>>;

/// 部署 owner 提供准备与停止绝对期限；这些参数不作为硬件完成证据。
#[derive(Clone, Copy)]
pub struct Policy {
    pub prepare_ns: u64,
    pub stop_ns: u64,
}
impl Policy {
    fn validate(self) -> Result<(), Error> {
        if self.prepare_ns == 0 || self.stop_ns == 0 {
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
    requirement: crate::binding::StopRequirement,
    // 唯一资源出口可能交出能力的不可逆事实；关闭别名/回复丢失不能清除。
    resources_exposed: bool,
    fault: Option<Error>,
    use_entry: Option<MailboxSender>,
    registrar: Option<(MailboxSender, MailboxSender)>,
    _unit: Charge,
    _bytes: Charge,
}

/// 服务端控制表及已接请求持有此共享观察；不是跨进程 capability。
/// 只有真实 sender context 通过角色表验证后才能调用控制方法。
#[derive(Clone)]
pub struct Binding(Rc<RefCell<Shared>>);
impl Binding {
    pub fn use_entry(&self) -> Result<Option<Capability>, Error> {
        self.0
            .borrow()
            .use_entry
            .as_ref()
            .map(sender_alias)
            .transpose()
    }
    pub fn install_use(&self, sender: MailboxSender) {
        let mut core = self.0.borrow_mut();
        assert!(core.use_entry.is_none(), "binding use already committed");
        core.use_entry = Some(sender);
    }
    pub fn registrar(&self) -> Result<Option<(MailboxSender, MailboxSender)>, Error> {
        self.0
            .borrow()
            .registrar
            .as_ref()
            .map(|(a, b)| Ok((sender_copy(a)?, sender_copy(b)?)))
            .transpose()
    }
    pub fn install_registrar(&self, entries: (MailboxSender, MailboxSender)) {
        let mut core = self.0.borrow_mut();
        assert!(
            core.registrar.is_none(),
            "binding registrar already committed"
        );
        core.registrar = Some(entries);
    }
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
    pub fn use_access(&self) -> Result<crate::binding::UseAccess, Error> {
        use crate::binding::{StopRequirement, UseAccess};
        let core = self.0.borrow();
        if core.requirement == StopRequirement::ConsumerQuiescence {
            if !core.can_report {
                return Err(Error::Permission);
            }
            Ok(UseAccess::RESOURCES.union(UseAccess::REPORT_QUIESCENT))
        } else {
            Ok(UseAccess::RESOURCES)
        }
    }
    pub fn stop_requirement(&self) -> crate::binding::StopRequirement {
        self.0.borrow().requirement
    }
    pub fn report_quiescent(&self, registry: &Backing) -> Result<u64, Error> {
        if !self
            .use_access()?
            .contains(crate::binding::UseAccess::REPORT_QUIESCENT)
        {
            return Err(Error::Permission);
        }
        match self.snapshot(registry)?.phase {
            Phase::Stopping => {}
            Phase::Released => return Err(Error::Closed),
            _ => return Err(Error::NotReady),
        }
        let mut core = self.0.borrow_mut();
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

/// 业务执行部分只活到 Released；旧角色尾部不再持供应能力或 RPC 状态。
struct Business {
    collecting: Option<Collecting<Capability>>,
    provider: Arc<MailboxSender>,
    signal: NotificationPair,
    source: SourceRegistration,
    policy: Policy,
    call: Option<(u64, ProvisionCommand)>,
    prepare_end: u64,
    stop_end: Option<u64>,
    prepare_sent: bool,
    stop_sent: Option<bool>,
    progress: Option<Progress>,
    next_get: Option<u32>,
    query_due: bool,
    wake_transport: bool,
    _storage: Charge,
}

pub struct Work {
    binding: Binding,
    funding: Funding,
    business: Option<Box<Business>>,
    committed: bool,
    retired: bool,
}
impl core::fmt::Debug for Work {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let core = self.binding.0.borrow();
        f.debug_struct("Work")
            .field("reference", &core.reference)
            .field("task", &core.task)
            .field("released", &core.released)
            .field("reported", &core.reported)
            .field("resources_exposed", &core.resources_exposed)
            .field("fault", &core.fault)
            .finish()
    }
}
impl Work {
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
            .acquire(DeviceResource::Bytes, core::mem::size_of::<Business>())?;
        let unit = funding.domain().acquire(DeviceResource::Binding, 1)?;
        let bytes = funding.domain().acquire(
            DeviceResource::Bytes,
            core::mem::size_of::<Shared>() + core::mem::size_of::<Self>(),
        )?;
        let signal = notification::create(
            Rights::READ | Rights::WAIT,
            Rights::SIGNAL | Rights::DUPLICATE | Rights::TRANSIT,
        )?;
        let binding = Binding(
            Rc::try_new(RefCell::new(Shared {
                reference,
                source,
                task: 0,
                resources: None,
                released: None,
                reported: false,
                can_report: registry.report_authorized(source, reference).is_ok(),
                requirement: registry.stop_requirement(reference.device())?,
                resources_exposed: false,
                fault: None,
                use_entry: None,
                registrar: None,
                _unit: unit,
                _bytes: bytes,
            }))
            .map_err(|_| Error::Resource)?,
        );
        let business = Box::try_new(Business {
            collecting: Some(collecting),
            provider: registry.provision(reference.device())?.clone(),
            signal,
            source: SourceRegistration::new(),
            policy,
            call: None,
            prepare_end: 0,
            stop_end: None,
            prepare_sent: false,
            stop_sent: None,
            progress: None,
            next_get: None,
            query_due: false,
            wake_transport: false,
            _storage: storage,
        })
        .map_err(|_| Error::Resource)?;
        Ok(Self {
            binding,
            funding,
            business: Some(business),
            committed: false,
            retired: false,
        })
    }
    pub fn funding(&self) -> &Funding {
        &self.funding
    }
    pub fn prepared_binding(&self) -> Binding {
        self.binding.clone()
    }
    pub fn binding(&self) -> Result<Binding, Error> {
        if !self.committed || self.retired {
            return Err(Error::NotReady);
        }
        Ok(self.binding.clone())
    }
    pub fn commit(&mut self, task: u64, now: u64, registry: &mut Backing) -> Result<Commit, Error> {
        if self.committed || task == 0 {
            return Err(Error::Invalid);
        }
        let business = self.business.as_mut().ok_or(Error::NotReady)?;
        let end = now
            .checked_add(business.policy.prepare_ns)
            .ok_or(Error::Exhausted)?;
        let core = self.binding.0.borrow();
        let (source, reference) = (core.source, core.reference);
        drop(core);
        registry.retain_source(source)?;
        if let Err(error) = registry.retain_device(reference.device()) {
            registry.release_source(source)?;
            return Err(error);
        }
        let result = registry.commit_claim(source, reference, task);
        if matches!(result, Ok(Commit::Created(_))) {
            self.binding.0.borrow_mut().task = task;
            business.prepare_end = end;
            self.committed = true;
        } else {
            registry.release_device(reference.device())?;
            registry.release_source(source)?;
        }
        result
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 5
            && let Some(business) = self.business.as_mut()
        {
            business.source.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        if let Some(business) = self.business.as_mut() {
            business.source.unregistered(id);
        }
    }
    pub fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        if event.kind == 5
            && let Some(business) = self.business.as_mut()
        {
            business.query_due = true;
        }
    }
    pub fn deadline(&self) -> Deadline {
        let Some(business) = &self.business else {
            return Deadline::INFINITE;
        };
        if self.has_fault() || !self.committed || business.call.is_some() {
            return Deadline::INFINITE;
        }
        if let Some(end) = business.stop_end {
            Deadline::at(end)
        } else if self.binding.0.borrow().resources.is_none() {
            Deadline::at(business.prepare_end)
        } else {
            Deadline::INFINITE
        }
    }
    pub fn has_fault(&self) -> bool {
        self.binding.0.borrow().fault.is_some()
    }
    pub fn transport_wake_failed(&mut self) {
        self.fault(Error::Unavailable);
    }
    fn fault(&self, error: Error) {
        self.binding.0.borrow_mut().fault.get_or_insert(error);
    }
    fn stop(&mut self, now: u64, registry: &mut Backing) -> Result<(), Error> {
        self.binding.stop(registry)?;
        let business = self.business.as_mut().ok_or(Error::Closed)?;
        if business.stop_end.is_none() {
            business.stop_end = Some(
                now.checked_add(business.policy.stop_ns)
                    .ok_or(Error::Exhausted)?,
            );
            let core = self.binding.0.borrow();
            libkernel::debug!(
                "device Binding stopping: reference={:?}, resources_exposed={}, reported={}",
                core.reference,
                core.resources_exposed,
                core.reported
            );
        }
        business.next_get = None;
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
        if let ProvisionCommand::Get { epoch, index } = command {
            if stopping {
                return self.stop(now, registry);
            }
            let entry =
                protocol::decode_resource(&reply.payload, epoch, index, reply.handles.remaining())?;
            let capability = reply.handles.take(0)?;
            entry.descriptor.check_handle(&capability.description()?)?;
            let business = self.business.as_mut().ok_or(Error::Closed)?;
            business
                .collecting
                .as_mut()
                .ok_or(Error::NotReady)?
                .push(&entry, capability)?;
            business.next_get = Some(index.checked_add(1).ok_or(Error::Exhausted)?);
            return Ok(());
        }
        let progress = Progress::decode(&reply.payload, command, reply.handles.remaining())?;
        let business = self.business.as_mut().ok_or(Error::Closed)?;
        if let ProvisionCommand::Stop {
            consumer_quiescent, ..
        } = command
        {
            business.stop_sent = Some(business.stop_sent.unwrap_or(false) || consumer_quiescent);
        }
        business.progress = Some(progress);
        match progress {
            Progress::Closed | Progress::Rejected(_) | Progress::Stopping | Progress::Failed(_) => {
                self.stop(now, registry)?
            }
            Progress::Prepared(count)
                if !stopping
                    && self.binding.0.borrow().resources.is_none()
                    && business.next_get.is_none() =>
            {
                business
                    .collecting
                    .as_mut()
                    .ok_or(Error::NotReady)?
                    .prepared(count)?;
                business.next_get = Some(0);
            }
            _ => {}
        }
        Ok(())
    }
    fn transport_error(
        &mut self,
        error: CallError,
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
        // Dispatcher 在交付前退休观察来源；未发送请求随 error 关闭，无回复口清理债务。
        // 失败不转为 Closed。首次提交/取资源未知仍由同轮 Stop 收束，停止或观察失联升级监督。
        if matches!(
            command,
            ProvisionCommand::Stop { .. } | ProvisionCommand::Query(_)
        ) {
            self.fault(Error::Unavailable);
        } else if let Err(error) = self.stop(now, registry) {
            self.fault(error);
        }
    }
    pub fn advance<F>(
        &mut self,
        registry: &mut Backing,
        dispatcher: &mut Dispatcher,
        transport_task: u64,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Advance {
        match self.drive(registry, dispatcher, transport_task, requests, input) {
            Ok(next) => step(next),
            Err(error) => {
                self.fault(error);
                step(Step::Parked)
            }
        }
    }
    fn drive<F>(
        &mut self,
        registry: &mut Backing,
        dispatcher: &mut Dispatcher,
        transport_task: u64,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Result<Step, Error> {
        let now = input.now_ns();
        while let Some(event) = input.pull() {
            self.observe(event);
        }
        input.take_timeout();
        if !self.committed || self.retired {
            return Ok(Step::Complete);
        }
        let Some(business) = self.business.as_mut() else {
            if Rc::strong_count(&self.binding.0) > 1 {
                return Ok(Step::Parked);
            }
            self.retired = true;
            return Ok(Step::Complete);
        };
        if business.wake_transport {
            requests.wake(transport_task)?;
            business.wake_transport = false;
        }
        if let Some((txid, command)) = business.call {
            if let Some(completion) = dispatcher.take(txid) {
                business.call = None;
                match completion.result {
                    Ok(reply) => {
                        if let Err(error) = self.consume(command, reply, now, registry) {
                            if matches!(
                                command,
                                ProvisionCommand::Stop { .. } | ProvisionCommand::Query(_)
                            ) {
                                return Err(error);
                            }
                            self.stop(now, registry)?;
                        }
                    }
                    Err(error) => self.transport_error(error, txid, command, now, registry),
                }
                return Ok(Step::Runnable);
            }
            let reference = self.binding.0.borrow().reference;
            if registry.binding_snapshot(reference)?.phase == Phase::Stopping
                && matches!(
                    command,
                    ProvisionCommand::Prepare(_) | ProvisionCommand::Get { .. }
                )
                && dispatcher.cancel(txid)
            {
                business.wake_transport = true;
                return Ok(Step::Runnable);
            }
            return Ok(Step::Parked);
        }
        if self.has_fault() {
            return Ok(Step::Parked);
        }
        let reference = self.binding.0.borrow().reference;
        let snapshot = self.binding.snapshot(registry)?;
        if snapshot.phase == Phase::Stopping {
            self.stop(now, registry)?;
        }
        let business = self.business.as_mut().ok_or(Error::Closed)?;
        if matches!(
            business.progress,
            Some(Progress::Closed | Progress::Rejected(_))
        ) {
            if snapshot.phase != Phase::Released {
                let released = match registry.release_binding(reference) {
                    Ok(released) => released,
                    Err(Error::Busy) if business.stop_end.is_none_or(|end| now < end) => {
                        return Ok(Step::Parked);
                    }
                    Err(error) => return Err(error),
                };
                let mut core = self.binding.0.borrow_mut();
                core.released = Some(released);
                core.resources = None;
                core.use_entry = None;
                core.registrar = None;
            }
            business.source.remove(requests)?;
            if !business.source.is_empty() {
                return Ok(Step::Parked);
            }
            let source = self.binding.0.borrow().source;
            registry.release_device(reference.device())?;
            registry.release_source(source)?;
            self.business = None;
            return Ok(Step::Runnable);
        }
        if business.stop_end.is_some_and(|end| now >= end) {
            return Err(Error::Unavailable);
        }
        if business.stop_end.is_none()
            && now >= business.prepare_end
            && self.binding.0.borrow().resources.is_none()
        {
            self.stop(now, registry)?;
            return Ok(Step::Runnable);
        }
        let business = self.business.as_mut().ok_or(Error::Closed)?;
        // 在 Prepare 及其可能的同步完成之前先安装等待来源。
        if business.source.is_empty() {
            business.source.install(
                requests,
                SourcePlan::new(business.signal.owner.as_handle(), ObjectSignals::READABLE),
                5,
            )?;
            return Ok(Step::Runnable);
        }
        if business.source.source().is_none() {
            return Ok(Step::Runnable);
        }
        let quiescent = {
            let core = self.binding.0.borrow();
            core.reported || !core.resources_exposed
        };
        let epoch = reference.epoch();
        let command = if business.stop_end.is_some()
            && (business.stop_sent.is_none() || (quiescent && business.stop_sent == Some(false)))
        {
            ProvisionCommand::Stop {
                epoch,
                consumer_quiescent: quiescent,
            }
        } else if business.stop_end.is_none() && !business.prepare_sent {
            ProvisionCommand::Prepare(epoch)
        } else if let Some(index) = business.next_get {
            let Some(Progress::Prepared(count)) = business.progress else {
                return Err(Error::Invalid);
            };
            if index == count {
                let collecting = business.collecting.take().ok_or(Error::NotReady)?;
                let frozen = collecting.finish().map_err(|_| Error::NotReady)?;
                match registry.resources_ready(reference) {
                    Ok(()) => {
                        self.binding.0.borrow_mut().resources = Some(frozen);
                        business.next_get = None;
                    }
                    // 祖先准入先于维护扫描封闭：原轮停止接管，残表不发表。
                    Err(Error::Closed) => self.stop(now, registry)?,
                    Err(error) => return Err(error),
                }
                return Ok(Step::Runnable);
            }
            ProvisionCommand::Get { epoch, index }
        } else if business.query_due {
            // 先消费提示；查询期间提交的后续提示仍在内核 Owner 上等待下一轮读取。
            match business.signal.owner.take(u64::MAX) {
                Ok(_) | Err(erhino_shared::call::SystemCallError::ObjectNotAvailable) => {}
                Err(error) => return Err(error.into()),
            }
            business.query_due = false;
            ProvisionCommand::Query(epoch)
        } else {
            return Ok(Step::Parked);
        };
        let deadline = Deadline::at(business.stop_end.unwrap_or(
            if self.binding.0.borrow().resources.is_some() {
                now.checked_add(business.policy.prepare_ns)
                    .ok_or(Error::Exhausted)?
            } else {
                business.prepare_end
            },
        ));
        let mut bytes = [0; protocol::HEADER_LEN + 12];
        let used = command.encode(deadline, &mut bytes)?;
        let mut request = Request::new(protocol::PROVISION_ID, &bytes[..used])?;
        if matches!(command, ProvisionCommand::Prepare(_)) {
            let handle = libkernel::ipc::object::duplicate(
                business.signal.peer.as_handle(),
                Rights::SIGNAL | Rights::TRANSIT,
            )?;
            // SAFETY: duplicate 返回的新 entry 只移交这次请求，供应者仅得 SIGNAL。
            request
                .push(unsafe { Capability::from_raw(handle) }, Rights::SIGNAL)
                .map_err(|failure| Error::from(failure.error))?;
            business.prepare_sent = true;
        }
        match dispatcher.begin_for(
            ServiceOwner::Shared(business.provider.clone()),
            deadline,
            request,
            Some(self.binding.task()),
            self.funding.execution(),
        ) {
            Ok(txid) => {
                business.call = Some((txid, command));
                business.wake_transport = true;
            }
            Err(failure) => self.transport_error(failure.error, 0, command, now, registry),
        }
        Ok(Step::Runnable)
    }
}
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}

fn sender_alias(sender: &MailboxSender) -> Result<Capability, Error> {
    let handle = libkernel::ipc::object::duplicate(
        sender.as_handle(),
        Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
    )?;
    // SAFETY: 新 entry 的唯一关闭责任移交给返回值。
    Ok(unsafe { Capability::from_raw(handle) })
}
fn sender_copy(sender: &MailboxSender) -> Result<MailboxSender, Error> {
    MailboxSender::from_capability(sender_alias(sender)?)
        .map(|(sender, _)| sender)
        .map_err(|failure| failure.error.into())
}
