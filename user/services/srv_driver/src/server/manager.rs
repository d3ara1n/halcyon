//! 有限设备扫描与唯一候选选择；每个实例的 Claim/激活由一份实际 Managed 任务承接。
use super::*;
use libdevice::{
    protocol::{self as device, Command, Header},
    registry::Scope,
};
use libexecution::runtime::SourcePlan;

/// 管理入口与公开观察入口的导出名。
const CONTROL_EXPORT: &str = "driver.control";
const OBSERVER_EXPORT: &str = "driver.observer";

#[derive(Clone, Copy)]
enum Operation {
    Watch,
    Query,
    Enumerate,
    Device(DeviceId),
    Retired(DeviceId),
    Ack,
}
pub(super) struct Manager {
    control: Option<MailboxSender>,
    observer: Option<MailboxSender>,
    watch: Option<libdevice::client::Watch>,
    source: SourceRegistration,
    call: Option<(u64, Operation)>,
    next: Option<Operation>,
    after: u64,
    end: u64,
    sequence: u64,
    wake_cursor: Option<u64>,
    stopping: bool,
    stop_notified: bool,
}
impl Manager {
    fn consider(
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        info: libdevice::registry::DeviceInfo<'_>,
    ) -> Result<(), SystemCallError> {
        if world.stopping || !info.present || info.binding.is_some() || info.epoch.is_none() {
            return Ok(());
        }
        let prior = world.runs.get(info.id.serial()).cloned();
        let previous = if let Some(record) = &prior {
            let run = record.borrow();
            if !run.can_fallback() {
                return Ok(());
            }
            Some(run.implementation)
        } else {
            None
        };
        let implementation = match world.catalog.select(info.identity, previous) {
            Ok(Some(index)) => index,
            Ok(None) => return Ok(()),
            Err(Error::Conflict) => {
                libkernel::debug!("driver selection ambiguous: device={}", info.id.serial());
                return Ok(());
            }
            Err(error) => fatal(error),
        };
        let replacement = Run {
            device: info.id,
            reference: BindingRef::new(info.id, info.epoch.expect("candidate epoch checked"))
                .unwrap_or_else(|e| fatal(e)),
            implementation,
            task: 0,
            process: None,
            released: false,
            stop_requested: false,
            failure: None,
            phase: protocol::Phase::Starting,
            binding_watch: 0,
            activation_watch: 0,
            binding_status: None,
            activation_status: None,
            watchers_stopping: false,
        };
        let rollback = prior.as_ref().map(|record| record.borrow().clone());
        let record = match &prior {
            Some(record) => record.clone(),
            None => Rc::try_new(RefCell::new(replacement.clone()))
                .map_err(|_| SystemCallError::OutOfMemory)?,
        };
        // 身份/任务/节点均先准备；复用原Run只发生在旧Managed和全部关联已退出之后。
        let task = Box::try_new(Managed::prepare(record.clone(), info.identity)?)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let entry = if prior.is_none() {
            Some(
                world
                    .runs
                    .prepare_insert(info.id.serial(), record.clone(), &AllocatorBacked)
                    .map_err(|_| SystemCallError::OutOfMemory)?,
            )
        } else {
            None
        };
        *record.borrow_mut() = replacement;
        if let Some(entry) = entry {
            world.runs.insert_prepared(entry);
        }
        if let Err(submission) = requests.spawn(TaskSubmission::new(
            Family::Managed(task),
            0,
            world.funding.clone(),
        )) {
            drop(submission);
            if let Some(rollback) = rollback {
                *record.borrow_mut() = rollback;
            } else {
                world.runs.remove(info.id.serial());
            }
        } else if let Some(previous) = rollback {
            let next = record.borrow();
            let previous_key = world
                .catalog
                .records()
                .nth(previous.implementation)
                .ok_or(SystemCallError::InternalError)?
                .unwrap_or_else(|e| fatal(e))
                .key;
            let next_key = world
                .catalog
                .records()
                .nth(next.implementation)
                .ok_or(SystemCallError::InternalError)?
                .unwrap_or_else(|e| fatal(e))
                .key;
            libkernel::debug!(
                "driver candidate advanced: device={}, from_epoch={}, to_epoch={}, from={}, to={}",
                info.id.serial(),
                previous.reference.epoch(),
                next.reference.epoch(),
                previous_key,
                next_key
            );
        }
        Ok(())
    }
    pub fn new(control: MailboxSender, observer: MailboxSender) -> Self {
        Self {
            control: Some(control),
            observer: Some(observer),
            watch: None,
            source: SourceRegistration::new(),
            call: None,
            next: Some(Operation::Watch),
            after: 0,
            end: 0,
            sequence: 0,
            wake_cursor: None,
            stopping: false,
            stop_notified: false,
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 1 {
            self.source.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.source.unregistered(id);
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.kind == 1 {
                world.discovery_dirty = true;
            }
        }
        input.take_timeout();
        if world.stopping || self.stopping {
            self.next = None;
            if let Some((txid, _)) = self.call {
                if world.rpc.cancel(txid) {
                    world.rpc_dirty = true;
                }
                let Some(completion) = world.rpc.take(txid) else {
                    return Ok(advance(Step::Parked));
                };
                // 停止已封新发现；原调用完成只兑现运输退休，不再接纳迟到扫描结果。
                drop(completion);
                self.call = None;
            }
        }
        if let Some((txid, operation)) = self.call {
            let Some(completion) = world.rpc.take(txid) else {
                return Ok(advance(Step::Parked));
            };
            self.call = None;
            let reply = completion
                .result
                .unwrap_or_else(|_| fatal(Error::Unavailable));
            match operation {
                Operation::Watch => {
                    self.watch = Some(
                        libdevice::client::Watch::from_reply(
                            reply,
                            world.claim.description()?.related_object_id,
                            15,
                        )
                        .unwrap_or_else(|error| fatal(error)),
                    );
                    self.next = None;
                }
                Operation::Query => {
                    let info = libdevice::client::Watch::decode_reply(&reply, 17)
                        .unwrap_or_else(|error| fatal(error));
                    if info.terminal() {
                        world.stopping = true;
                    }
                    self.sequence = info.sequence;
                    self.after = 0;
                    self.end = 0;
                    world.discovery_dirty = false;
                    self.next = Some(if world.stopping {
                        Operation::Ack
                    } else {
                        Operation::Enumerate
                    });
                }
                Operation::Enumerate => {
                    let (header, body) =
                        Header::decode(&reply.payload).unwrap_or_else(|error| fatal(error));
                    if header.op != 14 || reply.handles.remaining() != 0 {
                        fatal(Error::Invalid);
                    }
                    if header.error == Some(Error::Closed) && body.is_empty() {
                        // 来源封口可能先于 Scope Watch 提示到达，沿同一停止 owner 收束。
                        world.stopping = true;
                        self.next = None;
                        return Ok(advance(Step::Runnable));
                    }
                    if header.error.is_some() {
                        fatal(Error::Invalid);
                    }
                    let page = device::decode_scan(body, self.after, self.end)
                        .unwrap_or_else(|error| fatal(error));
                    self.after = page.next;
                    self.end = page.end;
                    self.next = Some(page.device.map_or(Operation::Ack, Operation::Device));
                }
                Operation::Device(device_id) => {
                    match device::decode_device(
                        &reply.payload,
                        device_id,
                        reply.handles.remaining(),
                    ) {
                        Ok(info) if info.present => {
                            Self::consider(world, requests, info)?;
                        }
                        Ok(info) if !info.present => {
                            if world
                                .runs
                                .get(info.id.serial())
                                .is_some_and(|r| r.borrow().task == 0)
                            {
                                world.runs.remove(info.id.serial());
                            }
                        }
                        Err(Error::StaleInstance) => {
                            // 有限枚举的原项已退休；只在发现循环清掉其已结束选择记录。
                            if world
                                .runs
                                .get(device_id.serial())
                                .is_some_and(|r| r.borrow().task == 0)
                            {
                                world.runs.remove(device_id.serial());
                            }
                        }
                        Ok(_) | Err(Error::NotFound | Error::Closed | Error::Permission) => {}
                        Err(error) => fatal(error),
                    }
                    self.next = Some(Operation::Enumerate);
                }
                Operation::Retired(device_id) => {
                    let retired = match device::decode_device(
                        &reply.payload,
                        device_id,
                        reply.handles.remaining(),
                    ) {
                        Ok(info) => {
                            let retired = !info.present;
                            if !retired {
                                Self::consider(world, requests, info)?;
                            }
                            retired
                        }
                        Err(Error::StaleInstance) => true,
                        Err(error) => fatal(error),
                    };
                    if retired
                        && world
                            .runs
                            .get(device_id.serial())
                            .is_some_and(|r| r.borrow().task == 0)
                    {
                        world.runs.remove(device_id.serial());
                    }
                }
                Operation::Ack => {
                    let info = libdevice::client::Watch::decode_reply(&reply, 18)
                        .unwrap_or_else(|error| fatal(error));
                    world.discovery_dirty |= info.pending != 0;
                    self.next = None;
                    self.wake_cursor = Some(0);
                }
            }
            return Ok(advance(Step::Runnable));
        }
        let retired = if let Some(cursor) = self.wake_cursor {
            if let Some((serial, record)) = world.runs.next_after(Some(&cursor)) {
                let serial = *serial;
                let (task, device) = {
                    let run = record.borrow();
                    (run.task, run.device)
                };
                self.wake_cursor = Some(serial);
                if task != 0 {
                    requests.wake(task)?;
                    return Ok(advance(Step::Runnable));
                }
                if world.stopping {
                    world.runs.remove(serial);
                    return Ok(advance(Step::Runnable));
                }
                // 纯选择结果已无执行/硬件责任；原发现变化时观察其身份，不创建定时扫描。
                Some(Operation::Retired(device))
            } else {
                self.wake_cursor = None;
                None
            }
        } else {
            None
        };
        if self.stopping {
            self.source.remove(requests)?;
            return Ok(advance(if self.source.is_empty() {
                Step::Complete
            } else {
                Step::Runnable
            }));
        }
        if world.stopping {
            self.source.remove(requests)?;
            if self.source.is_empty() {
                self.watch = None;
            }
            if !self.stop_notified {
                self.stop_notified = true;
                requests.wake(world.observer_ingress)?;
                self.wake_cursor = Some(0);
                return Ok(advance(Step::Runnable));
            }
            // 来源封口也会唤醒本任务；管理停止不等外部继续发 Query。
            if world.runs.is_empty()
                && world.processes.is_empty()
                && world.rpc.pending_count() == 0
                && world.outstanding_requests == 0
            {
                world.finished = true;
                // 外层下一轮接管 Runtime shutdown；本轮不能先进入无限 Wait。
                return Ok(advance(Step::Runnable));
            }
            return Ok(advance(if self.source.is_empty() {
                Step::Parked
            } else {
                Step::Runnable
            }));
        }
        if let Some(watch) = self.watch.as_ref()
            && self.source.is_empty()
        {
            self.source.install(
                requests,
                SourcePlan::new(
                    watch.observer().as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                1,
            )?;
            return Ok(advance(Step::Runnable));
        }
        if self.source.is_installing() {
            return Ok(advance(Step::Runnable));
        }
        if self.watch.is_some() && !world.ready {
            let reporter = world
                .reporter
                .as_mut()
                .ok_or(SystemCallError::ObjectClosed)?;
            for (name, sender) in [
                (CONTROL_EXPORT, &mut self.control),
                (OBSERVER_EXPORT, &mut self.observer),
            ] {
                let sender = sender.take().expect("driver startup entry missing");
                reporter
                    .export(
                        name,
                        sender.into_capability(),
                        TRANSPORT,
                        Deadline::INFINITE,
                    )
                    .map_err(|(_, error)| error)?;
            }
            reporter.ready(&[], Deadline::INFINITE)?;
            world.ready = true;
            libkernel::debug!(
                "driver manager ready; declarations={}, device Watch installed",
                world.catalog.count()
            );
        }
        let operation = retired.or(self.next).or(if world.discovery_dirty {
            Some(Operation::Query)
        } else {
            None
        });
        let Some(operation) = operation else {
            return Ok(advance(Step::Parked));
        };
        let command = match operation {
            Operation::Watch => Command::WatchScope { scope: Scope::Root },
            Operation::Query => Command::QueryWatch,
            Operation::Enumerate => Command::Enumerate {
                scope: Scope::Root,
                after: self.after,
                end: self.end,
            },
            Operation::Device(id) | Operation::Retired(id) => Command::QueryDevice { id },
            Operation::Ack => Command::AckWatch {
                sequence: self.sequence,
            },
        };
        let target = match operation {
            Operation::Query | Operation::Ack => Arc::try_new(
                alias(
                    self.watch
                        .as_ref()
                        .expect("driver Watch control missing")
                        .control(),
                )
                .unwrap_or_else(|error| fatal(error)),
            )
            .map_err(|_| SystemCallError::OutOfMemory)?,
            _ => world.claim.clone(),
        };
        let deadline = deadline()?;
        let mut bytes = [0; device::HEADER_LEN + device::BODY_MAX];
        let used = command
            .encode(deadline, &mut bytes)
            .unwrap_or_else(|error| fatal(error));
        let txid = world
            .call(
                id,
                target,
                Request::new(device::ID, &bytes[..used])?,
                deadline,
            )
            .unwrap_or_else(|_| fatal(Error::Unavailable));
        self.call = Some((txid, operation));
        self.next = None;
        Ok(advance(Step::Parked))
    }
}
