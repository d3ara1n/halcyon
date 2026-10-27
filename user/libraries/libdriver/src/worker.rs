//! 进程管理入口与逐Binding激活；程序组合原Runtime，库不创建运行体。
use crate::{
    activation::{Activation, Lifecycle},
    protocol::{self, Phase, Snapshot},
};

use alloc::sync::Arc;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Budget, Charge, Taxonomy};
use libdevice::{Error, identity::BindingRef, registry::EndpointIdentity};
use libexecution::{
    SourceRegistration,
    runtime::{Requests, SourceEvent, SourceId, SourcePlan},
};
use libkernel::ipc::{
    capability::Capability,
    message::{LifetimeObserver, Mailbox, MailboxSender},
    notification::{self, NotificationOwner, NotificationSignaler},
    object,
};
use librpc::outbox::Outbox;
pub const TRANSPORT: Rights = Rights::from_raw(
    Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub fn alias(sender: &MailboxSender) -> Result<Capability, Error> {
    let handle = object::duplicate(sender.as_handle(), TRANSPORT)?;
    // SAFETY: duplicate的新entry唯一关闭责任交给Capability。
    Ok(unsafe { Capability::from_raw(handle) })
}
#[derive(Clone, Copy)]
enum Resource {
    Active,
}
impl Taxonomy for Resource {
    const COUNT: usize = 1;
    fn slot(self) -> usize {
        0
    }
}
/// 唯一进程邮箱和认证管理入口；容量来自同次出生配置，不由运行中请求自报。
pub struct WorkerControl {
    pub mailbox: Mailbox,
    context: u64,
    identity: &'static [u8],
    child_registrar: bool,
    active: AccountView<Resource>,
    _budget: Arc<Budget>,
}
impl WorkerControl {
    pub fn new(
        identity: &'static [u8],
        child_registrar: bool,
        capacity: u32,
    ) -> Result<(Self, MailboxSender), Error> {
        if capacity == 0 {
            return Err(Error::Invalid);
        }
        let budget = Budget::new(&[capacity as usize], 1)?;
        let account = budget.account(&[capacity as usize])?;
        let active = account.view(&[budget.slot(0).ok_or(Error::Invalid)?])?;
        let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let entry = mailbox.mint(0, TRANSPORT)?.sender;
        let context = entry.description()?.object_id;
        Ok((
            Self {
                mailbox,
                context,
                identity,
                child_registrar,
                active,
                _budget: budget,
            },
            entry,
        ))
    }
    pub fn is_manager_context(&self, context: u64) -> bool {
        context == self.context
    }
    pub fn command<'a>(
        &self,
        context: u64,
        wire: &'a [u8],
        handles: usize,
    ) -> Result<protocol::Command<'a>, Error> {
        if !self.is_manager_context(context) {
            return Err(Error::Permission);
        }
        let (_, command) = protocol::Command::decode(wire, handles)?;
        if let protocol::Command::Activate {
            reference,
            identity,
            registrar,
        } = command
            && (identity != self.identity || registrar != self.child_registrar)
        {
            libkernel::debug!(
                "driver activation input rejected before resource acquisition: device={}, epoch={}, identity_match={}, registrar_match={}",
                reference.device().serial(),
                reference.epoch(),
                identity == self.identity,
                registrar == self.child_registrar
            );
            return Err(Error::Permission);
        }
        Ok(command)
    }
}
pub const OBSERVERS: usize = 8;
const LIFE: u64 = 0x1000;
pub const SLOT_SOURCES: usize = 2 + OBSERVERS;
struct Observed {
    life: LifetimeObserver,
    source: SourceRegistration,
    closed: bool,
}
impl Observed {
    fn new(life: LifetimeObserver) -> Self {
        Self {
            life,
            source: SourceRegistration::new(),
            closed: false,
        }
    }
    fn event(&mut self, event: SourceEvent) {
        if self.source.source() == Some(event.source)
            && event.observed.contains(ObjectSignals::CLOSED)
        {
            self.closed = true;
        }
    }
    fn advance<F>(
        &mut self,
        kind: u64,
        requests: &mut Requests<'_, F>,
    ) -> Result<bool, SystemCallError> {
        if self.closed {
            self.source.remove(requests)?;
        } else if self.source.is_empty() {
            self.source.install(
                requests,
                SourcePlan::new(self.life.as_handle(), ObjectSignals::CLOSED),
                kind,
            )?;
        }
        Ok(self.source.is_installing() || self.source.is_removing())
    }
    fn done(&self) -> bool {
        self.closed && self.source.is_empty()
    }
}
struct Observer {
    context: u64,
    lifetime: Observed,
    notification: NotificationOwner,
    signaler: NotificationSignaler,
    acknowledged: u64,
}
/// 原ref、活跃额度与独立导出身份；Released后仅保留付费终态，不复用旧context。
pub struct ActivationSlot {
    reference: BindingRef,
    pub(crate) activation: Option<Activation>,
    function: Option<MailboxSender>,
    function_identity: EndpointIdentity,
    function_life: Observed,
    observation: Option<MailboxSender>,
    observation_identity: EndpointIdentity,
    observation_life: Observed,
    observers: [Option<Observer>; OBSERVERS],
    active: Option<Charge>,
    lifecycle: Lifecycle,
}
impl ActivationSlot {
    pub fn new(worker: &WorkerControl, reference: BindingRef) -> Result<Self, Error> {
        let active = worker.active.acquire(Resource::Active, 1)?;
        let function = worker.mailbox.mint(0, TRANSPORT)?;
        let function_identity =
            EndpointIdentity::from_description(&function.sender.description()?)?;
        let observation = worker.mailbox.mint(0, TRANSPORT)?;
        let observation_identity =
            EndpointIdentity::from_description(&observation.sender.description()?)?;
        Ok(Self {
            reference,
            activation: None,
            function: Some(function.sender),
            function_identity,
            function_life: Observed::new(function.lifetime),
            observation: Some(observation.sender),
            observation_identity,
            observation_life: Observed::new(observation.lifetime),
            observers: [const { None }; OBSERVERS],
            active: Some(active),
            lifecycle: Lifecycle::Pending,
        })
    }
    /// 同 actor 的接纳点；首次输入由实例保存，消息不持远端准备查询。
    pub fn accept_activation(
        &mut self,
        worker: &WorkerControl,
        context: u64,
        wire: &[u8],
        handles: usize,
        outbox: &mut Outbox,
    ) -> Result<bool, Error> {
        let command = worker.command(context, wire, handles)?;
        if command.reference() != self.reference {
            return Err(Error::Conflict);
        }
        let protocol::Command::Activate {
            reference,
            identity,
            registrar,
        } = command
        else {
            return Err(Error::Invalid);
        };
        if !self.active() {
            return Err(Error::Closed);
        }
        let (request, _) = outbox.response_mut()?.parts()?;
        let mut sender = |index| -> Result<alloc::sync::Arc<MailboxSender>, Error> {
            let (entry, description) = MailboxSender::from_capability(request.handles.take(index)?)
                .map_err(|failure| failure.error)?;
            if !description.rights.contains(TRANSPORT) {
                return Err(Error::Permission);
            }
            alloc::sync::Arc::try_new(entry).map_err(|_| Error::Resource)
        };
        let usage = sender(1)?;
        let registrar = if registrar { Some(sender(2)?) } else { None };
        if let Some(registrar) = &registrar {
            let a = EndpointIdentity::from_description(&usage.description()?)?;
            let b = EndpointIdentity::from_description(&registrar.description()?)?;
            if a == b || a.mailbox != b.mailbox {
                return Err(Error::Permission);
            }
        }
        if let Some(activation) = &self.activation {
            activation.same_input(reference, identity, &usage, registrar.as_deref())?;
            Ok(false)
        } else {
            self.activation = Some(Activation::prepare(reference, identity, usage, registrar)?);
            Ok(true)
        }
    }
    /// 认证由 WorkerControl 完成；结果读取不接纳输入、不改变阶段、不通知执行任务。
    pub fn query_result(&self, reference: BindingRef) -> Result<(), Error> {
        if reference != self.reference {
            return Err(Error::Conflict);
        }
        self.activation.as_ref().ok_or(Error::NotFound).map(|_| ())
    }
    pub fn activation(&self) -> Option<&Activation> {
        self.activation.as_ref()
    }
    pub fn io_ready(&self) -> bool {
        self.lifecycle.io_ready()
    }
    pub fn lifecycle_task(&self) -> Option<u64> {
        self.lifecycle.task()
    }
    pub(crate) fn attach_lifecycle(&mut self, task: u64) {
        self.lifecycle.attach(task);
    }
    pub(crate) fn allow_io(&mut self) -> Result<(), Error> {
        assert!(
            self.active() && self.activation.is_some(),
            "inactive activation admitted IO"
        );
        self.lifecycle.allow_io()
    }
    pub(crate) fn retire_lifecycle(&mut self) {
        if !self.io_ready() {
            self.retire_preparing_access();
        }
        self.lifecycle.retire();
    }
    pub fn ready(&mut self) -> Result<(), Error> {
        self.activation.as_mut().ok_or(Error::NotReady)?.ready()?;
        self.changed()
    }
    pub fn stop(&mut self) -> Result<(), Error> {
        let activation = self.activation.as_mut().ok_or(Error::NotReady)?;
        if matches!(activation.snapshot().phase, Phase::Starting | Phase::Ready) {
            activation.stop();
            self.changed()?;
        }
        Ok(())
    }
    pub fn fail(&mut self, error: Error) -> Result<(), Error> {
        self.activation.as_mut().ok_or(Error::NotReady)?.fail(error);
        self.changed()
    }
    pub fn terminal(&mut self, error: Option<Error>) -> Result<(), Error> {
        self.activation
            .as_mut()
            .ok_or(Error::NotReady)?
            .terminal(error);
        self.changed()
    }
    /// # Safety
    /// 原 Engine 已永久停止本轮及其委派访问，全部在途硬件责任已结束。
    pub unsafe fn consumer_stopped(&mut self) -> Result<(), Error> {
        // SAFETY: 由实际 Engine 按本方法的同一证据合同提交。
        unsafe {
            self.activation
                .as_mut()
                .ok_or(Error::NotReady)?
                .consumer_stopped()?;
        }
        self.changed()
    }
    /// 未获 IO 许可的准备失败，原生命周期任务仍负责在途查询和观察注销。
    pub fn reject_preparing(&mut self, error: Error) -> Result<(), Error> {
        assert!(!self.io_ready(), "active driver rejected as preparation");
        if let Some(activation) = self.activation.as_mut() {
            activation.terminal(Some(error));
        }
        self.active = None;
        self.function = None;
        self.observation = None;
        self.changed()
    }
    pub(crate) fn retire_preparing_access(&mut self) {
        assert!(!self.io_ready(), "active driver retired as preparation");
        if let Some(activation) = self.activation.as_mut() {
            activation.retire_unstarted_access();
        }
    }
    pub fn reference(&self) -> BindingRef {
        self.reference
    }
    pub fn active(&self) -> bool {
        self.active.is_some()
    }
    pub fn is_function_context(&self, context: u64) -> bool {
        context == self.function_identity.object.get()
    }
    pub fn is_observation_context(&self, context: u64) -> bool {
        context == self.observation_identity.object.get()
            || self
                .observers
                .iter()
                .flatten()
                .any(|o| o.context == context)
    }
    pub fn changed(&self) -> Result<(), Error> {
        let Some(activation) = &self.activation else {
            return Ok(());
        };
        let sequence = activation.snapshot().sequence;
        for observer in self.observers.iter().flatten() {
            if observer.acknowledged != sequence {
                observer.signaler.signal(1)?;
            } else {
                match observer.notification.take(u64::MAX) {
                    Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
    /// 仅在尚未提交Activation时拒绝准备；没有消费者访问效果，不伪造Binding Released。
    pub fn reject_uncommitted(&mut self) {
        if self.activation.is_none() {
            self.active = None;
            self.function = None;
            self.observation = None;
        }
    }
    /// caller已取完原Use查询/本轮工作；外部Closed错误不能代替这个真实Released快照。
    pub fn release(&mut self, snapshot: libdevice::binding::Snapshot) -> Result<(), Error> {
        self.activation
            .as_mut()
            .ok_or(Error::NotReady)?
            .retire_access(snapshot)?;
        self.active = None;
        self.function = None;
        self.observation = None;
        Ok(())
    }
    pub fn receipt(
        &self,
        outbox: &mut Outbox,
        op: u16,
        name: &str,
        description: &[u8],
    ) -> Result<(), Error> {
        let status = self.activation.as_ref().ok_or(Error::NotReady)?.snapshot();
        let ready = op == 2 && status.phase == Phase::Ready;
        let mut body = [0; protocol::BODY_MAX];
        let used = protocol::encode_receipt(
            status,
            if ready { name } else { "" },
            if ready { description } else { &[] },
            &mut body,
        )?;
        let response = outbox.response_mut()?;
        if op == 1 {
            response
                .push(
                    alias(self.observation.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )
                .map_err(|failure| failure.error)?;
        }
        if ready {
            response
                .push(
                    alias(self.function.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )
                .map_err(|f| f.error)?;
        }
        let length = protocol::Header {
            op,
            error: None,
            deadline: Deadline::INFINITE,
        }
        .encode(&body[..used], response.body_mut()?)?;
        response.finish_body(length)?;
        Ok(())
    }
    pub fn observe(
        &mut self,
        worker: &WorkerControl,
        context: u64,
        wire: &[u8],
        handles: usize,
        outbox: &mut Outbox,
    ) -> Result<(), Error> {
        let (header, command) = protocol::Observe::decode(wire, handles)?;
        let mut status = self.activation.as_ref().ok_or(Error::NotReady)?.snapshot();
        match command {
            protocol::Observe::Watch => {
                if context != self.observation_identity.object.get() {
                    return Err(Error::Permission);
                }
                if !self.active() {
                    return Err(Error::Closed);
                }
                let index = self
                    .observers
                    .iter()
                    .position(Option::is_none)
                    .ok_or(Error::Quota)?;
                let control = worker.mailbox.mint(0, TRANSPORT)?;
                let identity = control.sender.description()?.object_id;
                let pair = notification::create(
                    Rights::READ | Rights::WAIT | Rights::MANAGE,
                    Rights::SIGNAL,
                )?;
                let rights = Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT;
                let observer = pair.owner.mint_observer(rights)?;
                self.observers[index] = Some(Observer {
                    context: identity,
                    lifetime: Observed::new(control.lifetime),
                    notification: pair.owner,
                    signaler: pair.peer,
                    acknowledged: 0,
                });
                let response = outbox.response_mut()?;
                response
                    .push(control.sender.into_capability(), TRANSPORT)
                    .map_err(|f| f.error)?;
                response
                    .push(observer.into_capability(), rights)
                    .map_err(|f| f.error)?;
                status.pending = true;
            }
            protocol::Observe::Query if context == self.observation_identity.object.get() => {}
            protocol::Observe::Query | protocol::Observe::Ack(_) => {
                let observer = self
                    .observers
                    .iter_mut()
                    .flatten()
                    .find(|o| o.context == context)
                    .ok_or(Error::Permission)?;
                if let protocol::Observe::Ack(sequence) = command {
                    if sequence == 0 || sequence > status.sequence {
                        return Err(Error::Invalid);
                    }
                    observer.acknowledged = observer.acknowledged.max(sequence);
                }
                status.pending = observer.acknowledged != status.sequence;
            }
        }
        let mut body = [0; Snapshot::BYTES];
        let used = status.encode(&mut body)?;
        let response = outbox.response_mut()?;
        let length = protocol::Header {
            op: header.op,
            error: None,
            deadline: Deadline::INFINITE,
        }
        .encode(&body[..used], response.body_mut()?)?;
        response.finish_body(length)?;
        self.changed()
    }
    pub fn event(&mut self, event: SourceEvent) {
        match event.kind {
            LIFE => self.function_life.event(event),
            n if n == LIFE + 1 => self.observation_life.event(event),
            n if n >= LIFE + 2 && n < LIFE + SLOT_SOURCES as u64 => {
                if let Some(observer) = &mut self.observers[(n - LIFE - 2) as usize] {
                    observer.lifetime.event(event);
                }
            }
            _ => {}
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        match kind {
            LIFE => self.function_life.source.registered(kind, id),
            n if n == LIFE + 1 => self.observation_life.source.registered(kind, id),
            n if n >= LIFE + 2 && n < LIFE + SLOT_SOURCES as u64 => {
                if let Some(observer) = &mut self.observers[(n - LIFE - 2) as usize] {
                    observer.lifetime.source.registered(kind, id);
                }
            }
            _ => {}
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.function_life.source.unregistered(id);
        self.observation_life.source.unregistered(id);
        for observer in self.observers.iter_mut().flatten() {
            observer.lifetime.source.unregistered(id);
        }
    }
    pub fn advance_lifetimes<F>(
        &mut self,
        requests: &mut Requests<'_, F>,
    ) -> Result<bool, SystemCallError> {
        // 每步至多提交一次来源变更；关闭风暴也不溢出Runtime的原Gate操作预算。
        if self.function_life.advance(LIFE, requests)? {
            return Ok(true);
        }
        if self.observation_life.advance(LIFE + 1, requests)? {
            return Ok(true);
        }
        for (index, entry) in self.observers.iter_mut().enumerate() {
            if let Some(observer) = entry {
                if observer
                    .lifetime
                    .advance(LIFE + 2 + index as u64, requests)?
                {
                    return Ok(true);
                }
                if observer.lifetime.done() {
                    *entry = None;
                }
            }
        }
        Ok(false)
    }
    pub fn tail_done(&self) -> bool {
        !self.active()
            && self.lifecycle.retired()
            && self.function_life.done()
            && self.observation_life.done()
            && self.observers.iter().all(Option::is_none)
    }
}
