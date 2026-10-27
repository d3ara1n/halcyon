//! 当前键盘状态与独立Watch游标；原实例任务持有实际关闭观察至退休。
use super::*;
use libkernel::ipc::{
    message::LifetimeObserver,
    notification::{self, NotificationOwner, NotificationSignaler},
};
use librpc::outbox::Outbox;
pub(super) const WATCHES: usize = 8;
const LIFE: u64 = 0x2000;
struct Watch {
    context: u64,
    life: LifetimeObserver,
    source: SourceRegistration,
    closed: bool,
    notification: NotificationOwner,
    signaler: NotificationSignaler,
    ack: u64,
}
pub(super) struct Keys {
    pub state: libhid::Keyboard,
    watches: [Option<Watch>; WATCHES],
}
impl Keys {
    pub fn new() -> Self {
        Self {
            state: libhid::Keyboard::new(),
            watches: [const { None }; WATCHES],
        }
    }
    pub fn has_context(&self, context: u64) -> bool {
        self.watches.iter().flatten().any(|w| w.context == context)
    }
    pub fn is_empty(&self) -> bool {
        self.watches.iter().all(Option::is_none)
    }
    pub fn changed(&self) -> Result<(), Error> {
        let seq = self.state.snapshot().sequence;
        for watch in self.watches.iter().flatten() {
            if watch.ack != seq {
                watch.signaler.signal(1)?;
            } else {
                match watch.notification.take(u64::MAX) {
                    Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
    pub fn event(&mut self, event: libexecution::runtime::SourceEvent) {
        if let Some(watch) = self
            .watches
            .iter_mut()
            .flatten()
            .find(|w| w.source.source() == Some(event.source))
            && event.observed.contains(ObjectSignals::CLOSED)
        {
            watch.closed = true;
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if (LIFE..LIFE + WATCHES as u64).contains(&kind) {
            self.watches[(kind - LIFE) as usize]
                .as_mut()
                .expect("HID Watch installation owner missing")
                .source
                .registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        for w in self.watches.iter_mut().flatten() {
            w.source.unregistered(id);
        }
    }
    pub fn advance(
        &mut self,
        requests: &mut Requests<'_, Family>,
    ) -> Result<bool, SystemCallError> {
        for (index, entry) in self.watches.iter_mut().enumerate() {
            if let Some(watch) = entry {
                if watch.closed {
                    watch.source.remove(requests)?;
                } else if watch.source.is_empty() {
                    watch.source.install(
                        requests,
                        SourcePlan::new(watch.life.as_handle(), ObjectSignals::CLOSED),
                        LIFE + index as u64,
                    )?;
                }
                if watch.source.is_installing() || watch.source.is_removing() {
                    return Ok(true);
                }
                if watch.closed && watch.source.is_empty() {
                    *entry = None;
                }
            }
        }
        Ok(false)
    }
    pub fn request(
        &mut self,
        context: u64,
        command: libhid::protocol::Command,
        function: bool,
        mailbox: &Mailbox,
        outbox: &mut Outbox,
    ) -> Result<(), Error> {
        use libhid::protocol::Command;
        let snapshot = self.state.snapshot();
        let pending = match command {
            Command::Watch => {
                if !function {
                    return Err(Error::Permission);
                }
                if !snapshot.connected {
                    return Err(Error::Closed);
                }
                let index = self
                    .watches
                    .iter()
                    .position(Option::is_none)
                    .ok_or(Error::Quota)?;
                let control = mailbox.mint(0, TRANSPORT)?;
                let id = control.sender.description()?.object_id;
                let pair = notification::create(
                    erhino_shared::object::Rights::READ
                        | erhino_shared::object::Rights::WAIT
                        | erhino_shared::object::Rights::MANAGE,
                    erhino_shared::object::Rights::SIGNAL,
                )?;
                let rights = erhino_shared::object::Rights::WAIT
                    | erhino_shared::object::Rights::DUPLICATE
                    | erhino_shared::object::Rights::TRANSIT;
                let observer = pair.owner.mint_observer(rights)?;
                // 固定槽存储与原实例来源额度在接受前已付；先接管寿命再运输真实sender。
                self.watches[index] = Some(Watch {
                    context: id,
                    life: control.lifetime,
                    source: SourceRegistration::new(),
                    closed: false,
                    notification: pair.owner,
                    signaler: pair.peer,
                    ack: 0,
                });
                let response = outbox.response_mut()?;
                response
                    .push(control.sender.into_capability(), TRANSPORT)
                    .map_err(|f| f.error)?;
                response
                    .push(observer.into_capability(), rights)
                    .map_err(|f| f.error)?;
                true
            }
            Command::Query if function => false,
            Command::Query | Command::Ack(_) => {
                let watch = self
                    .watches
                    .iter_mut()
                    .flatten()
                    .find(|w| w.context == context)
                    .ok_or(Error::Permission)?;
                if let Command::Ack(seq) = command {
                    if seq == 0 || seq > snapshot.sequence {
                        return Err(Error::Invalid);
                    }
                    watch.ack = watch.ack.max(seq);
                }
                watch.ack != snapshot.sequence
            }
        };
        let mut body = [0; libhid::Snapshot::BYTES];
        snapshot.encode(pending, &mut body)?;
        let response = outbox.response_mut()?;
        let length = libhid::protocol::Header {
            op: command.op(),
            error: None,
            deadline: Deadline::INFINITE,
        }
        .encode(&body, response.body_mut()?)?;
        response.finish_body(length)?;
        self.changed()
    }
}
