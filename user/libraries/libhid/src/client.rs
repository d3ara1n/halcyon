//! 键盘功能客户端；实际订阅仍由librpc原ConsumerTask推进。
use crate::{
    Snapshot,
    protocol::{self, Command},
};
use erhino_shared::{object::Rights, time::Deadline};
use libdevice::Error;
use libkernel::ipc::{message::MailboxSender, notification::NotificationObserver};
use librpc::subscription::{Failure, Protocol, SnapshotState};
#[derive(Clone, Copy)]
pub struct Observation {
    pub state: Snapshot,
    pub pending: bool,
}
pub struct Watch {
    control: MailboxSender,
    observer: NotificationObserver,
    info: Observation,
}
impl Watch {
    pub fn from_reply(mut reply: librpc::Reply, function: &MailboxSender) -> Result<Self, Error> {
        let info = Self::decode_reply(&reply, 1, 2)?;
        let (control, description) =
            MailboxSender::from_capability(reply.handles.take(0)?).map_err(|f| f.error)?;
        let parent = function.description()?;
        if description.related_object_id != parent.related_object_id
            || description.object_id == parent.object_id
            || !description
                .rights
                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        {
            return Err(Error::Permission);
        }
        let (observer, _) =
            NotificationObserver::from_capability(reply.handles.take(1)?).map_err(|f| f.error)?;
        Ok(Self {
            control,
            observer,
            info,
        })
    }
    pub fn info(&self) -> Observation {
        self.info
    }
    pub fn control(&self) -> &MailboxSender {
        &self.control
    }
    pub fn observer(&self) -> &NotificationObserver {
        &self.observer
    }
    pub fn decode_reply(
        reply: &librpc::Reply,
        op: u16,
        handles: usize,
    ) -> Result<Observation, Error> {
        let (header, body) = protocol::Header::decode(&reply.payload)?;
        if header.op != op || reply.handles.remaining() != handles {
            return Err(Error::Invalid);
        }
        if let Some(error) = header.error {
            if !body.is_empty() || handles != 0 {
                return Err(Error::Invalid);
            }
            return Err(error);
        }
        let (state, pending) = Snapshot::decode(body)?;
        Ok(Observation { state, pending })
    }
}
impl Protocol for Watch {
    type Snapshot = Observation;
    type Status = Error;
    fn into_parts(self) -> (MailboxSender, NotificationObserver, Observation) {
        (self.control, self.observer, self.info)
    }
    fn request(
        command: librpc::subscription::Command,
        deadline: Deadline,
    ) -> Result<librpc::Request, Failure<Error>> {
        let command = match command {
            librpc::subscription::Command::Query => Command::Query,
            librpc::subscription::Command::Ack(seq) => Command::Ack(seq),
        };
        let mut wire = [0; protocol::HEADER_LEN + 8];
        let length = command
            .encode(deadline, &mut wire)
            .map_err(Failure::Status)?;
        librpc::Request::new(protocol::ID, &wire[..length]).map_err(Failure::System)
    }
    fn response(
        reply: &librpc::Reply,
        command: librpc::subscription::Command,
        _: Deadline,
    ) -> Result<Observation, Failure<Error>> {
        let op = match command {
            librpc::subscription::Command::Query => 2,
            librpc::subscription::Command::Ack(_) => 3,
        };
        Self::decode_reply(reply, op, 0).map_err(Failure::Status)
    }
    fn observation(info: &Observation) -> SnapshotState {
        SnapshotState {
            sequence: info.state.sequence,
            pending: info.pending,
            terminal: !info.state.connected,
        }
    }
}
