//! 进程管理入口与逐Binding激活；程序组合原Runtime，库不创建运行体。
use crate::{
    activation::Activation,
    protocol::{self, Phase, Snapshot},
};
use alloc::{sync::Arc, vec::Vec};
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
use librpc::{
    dispatcher::{Dispatcher, ServiceOwner},
    outbox::Outbox,
};
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
    pub activation: Option<Activation>,
    function: Option<MailboxSender>,
    function_identity: EndpointIdentity,
    function_life: Observed,
    observation: Option<MailboxSender>,
    observation_identity: EndpointIdentity,
    observation_life: Observed,
    observers: [Option<Observer>; OBSERVERS],
    active: Option<Charge>,
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
        })
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
        let observation = self.observation.as_ref().ok_or(Error::Closed)?;
        let ready = status.phase == Phase::Ready;
        let mut body = [0; protocol::BODY_MAX];
        let used = protocol::encode_receipt(
            status,
            if ready { name } else { "" },
            if ready { description } else { &[] },
            &mut body,
        )?;
        let response = outbox.response_mut()?;
        response
            .push(alias(observation)?, TRANSPORT)
            .map_err(|f| f.error)?;
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
            && self.function_life.done()
            && self.observation_life.done()
            && self.observers.iter().all(Option::is_none)
    }
}
pub struct Activate {
    reference: BindingRef,
    identity: Vec<u8>,
    usage: Arc<MailboxSender>,
    registrar: Option<Arc<MailboxSender>>,
    stop: bool,
    query: Option<u64>,
}
impl Activate {
    pub fn adopt(
        worker: &WorkerControl,
        slot: &ActivationSlot,
        context: u64,
        wire: &[u8],
        handles: usize,
        outbox: &mut Outbox,
    ) -> Result<Self, Error> {
        if !worker.is_manager_context(context) {
            return Err(Error::Permission);
        }
        let (_, command) = protocol::Command::decode(wire, handles)?;
        let reference = command.reference();
        if reference != slot.reference() {
            return Err(Error::Conflict);
        }
        if !slot.active() && slot.activation.is_none() {
            return Err(Error::Closed);
        }
        let stop = matches!(command, protocol::Command::Stop { .. });
        let (identity, has_registrar) = match command {
            protocol::Command::Activate {
                identity,
                registrar,
                ..
            } => (identity, registrar),
            _ => (&[][..], false),
        };
        if !stop && (identity != worker.identity || has_registrar != worker.child_registrar) {
            libkernel::debug!(
                "driver activation input rejected before resource acquisition: device={}, epoch={}, identity_match={}, registrar_match={}",
                reference.device().serial(),
                reference.epoch(),
                identity == worker.identity,
                has_registrar == worker.child_registrar
            );
            return Err(Error::Permission);
        }
        let mut frozen = Vec::new();
        frozen
            .try_reserve_exact(identity.len())
            .map_err(|_| Error::Resource)?;
        frozen.extend_from_slice(identity);
        let (request, _) = outbox.response_mut()?.parts()?;
        let mut sender = |index| -> Result<Arc<MailboxSender>, Error> {
            let (entry, description) = MailboxSender::from_capability(request.handles.take(index)?)
                .map_err(|f| f.error)?;
            if !description.rights.contains(TRANSPORT) {
                return Err(Error::Permission);
            }
            Arc::try_new(entry).map_err(|_| Error::Resource)
        };
        let usage = sender(1)?;
        let registrar = if has_registrar {
            Some(sender(2)?)
        } else {
            None
        };
        if let Some(registrar) = &registrar {
            let a = EndpointIdentity::from_description(&usage.description()?)?;
            let b = EndpointIdentity::from_description(&registrar.description()?)?;
            if a == b || a.mailbox != b.mailbox {
                return Err(Error::Permission);
            }
        }
        Ok(Self {
            reference,
            identity: frozen,
            usage,
            registrar,
            stop,
            query: None,
        })
    }
    pub fn advance(
        &mut self,
        slot: &mut ActivationSlot,
        rpc: &mut Dispatcher,
        task: u64,
        deadline: Deadline,
        funding: &libbudget::AccountView<libexecution::ExecutionResource>,
    ) -> Result<bool, Error> {
        if self.reference != slot.reference() {
            return Err(Error::Conflict);
        }
        if let Some(txid) = self.query {
            let Some(result) = rpc.take(txid) else {
                return Ok(false);
            };
            self.query = None;
            let reply = result.result.map_err(|_| Error::Unavailable)?;
            let (header, body) = libdevice::protocol::Header::decode(&reply.payload)?;
            if header.op != 8 || reply.handles.remaining() != 0 {
                return Err(Error::Invalid);
            }
            if let Some(error) = header.error {
                return Err(error);
            }
            let snapshot = libdevice::protocol::decode_snapshot(body)?;
            if slot.activation.is_none() {
                if !slot.active() {
                    return Err(Error::Closed);
                }
                slot.activation = Some(if self.stop {
                    Activation::cancel(self.reference, self.usage.clone(), snapshot)?
                } else {
                    Activation::prepare(
                        self.reference,
                        &self.identity,
                        self.usage.clone(),
                        self.registrar.clone(),
                        snapshot,
                    )?
                });
            }
        }
        if let Some(activation) = slot.activation.as_mut() {
            if self.stop {
                if activation.reference() != self.reference || !activation.same_use(&self.usage)? {
                    return Err(Error::Permission);
                }
                activation.stop();
            } else {
                activation.same_input(
                    self.reference,
                    &self.identity,
                    &self.usage,
                    self.registrar.as_deref(),
                )?;
            }
            slot.changed()?;
            return Ok(true);
        }
        let mut bytes = [0; libdevice::protocol::HEADER_LEN];
        let used = libdevice::protocol::Command::QueryBinding.encode(deadline, &mut bytes)?;
        self.query = Some(
            rpc.begin_for(
                ServiceOwner::Shared(self.usage.clone()),
                deadline,
                librpc::Request::new(libdevice::protocol::ID, &bytes[..used])?,
                Some(task),
                funding,
            )
            .map_err(|_| Error::Unavailable)?,
        );
        Ok(false)
    }
    pub fn retire(&mut self, rpc: &mut Dispatcher) -> bool {
        if let Some(txid) = self.query {
            if rpc.take(txid).is_none() {
                return false;
            }
            self.query = None;
        }
        true
    }
}
