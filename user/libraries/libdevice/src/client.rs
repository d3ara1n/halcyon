//! 设备观察客户端；复用 RPC 订阅会话，不启动运行体。
use crate::{
    Error,
    protocol::{self, Command},
    watch::Info,
};
use erhino_shared::{object::Rights, time::Deadline};
use libkernel::ipc::{message::MailboxSender, notification::NotificationObserver};
use librpc::subscription::{Failure, Protocol, SnapshotState};

pub struct Watch {
    control: MailboxSender,
    observer: NotificationObserver,
    info: Info,
}
impl Watch {
    /// 采用实际 WatchScope/WatchBinding 回复，校验槽数、角色及所属 registry。
    pub fn from_reply(mut reply: librpc::Reply, registry: u64, op: u16) -> Result<Self, Error> {
        if !matches!(op, 15 | 16) || registry == 0 {
            return Err(Error::Invalid);
        }
        let info = protocol::decode_watch(&reply.payload, op, reply.handles.remaining())?;
        let (control, description) = MailboxSender::from_capability(reply.handles.take(0)?)
            .map_err(|f| Error::from(f.error))?;
        if description.related_object_id != registry
            || !description
                .rights
                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        {
            return Err(Error::Permission);
        }
        let (observer, _) = NotificationObserver::from_capability(reply.handles.take(1)?)
            .map_err(|f| Error::from(f.error))?;
        Ok(Self {
            control,
            observer,
            info,
        })
    }
    pub fn info(&self) -> Info {
        self.info
    }
    pub fn control(&self) -> &MailboxSender {
        &self.control
    }
    pub fn observer(&self) -> &NotificationObserver {
        &self.observer
    }
    pub fn decode_reply(reply: &librpc::Reply, op: u16) -> Result<Info, Error> {
        if !matches!(op, 17..=19) {
            return Err(Error::Invalid);
        }
        protocol::decode_watch(&reply.payload, op, reply.handles.remaining())
    }
}
impl Protocol for Watch {
    type Snapshot = Info;
    type Status = Error;
    fn into_parts(self) -> (MailboxSender, NotificationObserver, Info) {
        (self.control, self.observer, self.info)
    }
    fn request(
        command: librpc::subscription::Command,
        deadline: Deadline,
    ) -> Result<librpc::Request, Failure<Error>> {
        let command = match command {
            librpc::subscription::Command::Query => Command::QueryWatch,
            librpc::subscription::Command::Ack(sequence) => Command::AckWatch { sequence },
        };
        let mut bytes = [0; protocol::HEADER_LEN + 8];
        let used = command
            .encode(deadline, &mut bytes)
            .map_err(Failure::Status)?;
        librpc::Request::new(protocol::ID, &bytes[..used]).map_err(Failure::System)
    }
    fn response(
        reply: &librpc::Reply,
        command: librpc::subscription::Command,
        _deadline: Deadline,
    ) -> Result<Info, Failure<Error>> {
        let op = match command {
            librpc::subscription::Command::Query => 17,
            librpc::subscription::Command::Ack(_) => 18,
        };
        Self::decode_reply(reply, op).map_err(Failure::Status)
    }
    fn observation(info: &Info) -> SnapshotState {
        SnapshotState {
            sequence: info.sequence,
            pending: info.pending != 0,
            terminal: info.terminal(),
        }
    }
}
