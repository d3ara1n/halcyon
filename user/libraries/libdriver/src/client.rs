//! 激活结果与观察客户端；实际调用由消费者已有的 Dispatcher/Runtime 承担。
use crate::protocol::{self, Header, Observe, Phase, Snapshot};
use alloc::string::String;
use alloc::vec::Vec;
use erhino_shared::object::Rights;
use erhino_shared::time::Deadline;
use libdevice::Error;
use libkernel::ipc::{message::MailboxSender, notification::NotificationObserver};
use librpc::subscription::{Failure, Protocol, SnapshotState};

pub struct Receipt {
    pub snapshot: Snapshot,
    pub observation: Option<MailboxSender>,
    pub function: Option<MailboxSender>,
    pub description: Vec<u8>,
    pub name: String,
}
impl Receipt {
    pub fn decode(
        mut reply: librpc::Reply,
        expected: libdevice::identity::BindingRef,
        worker_mailbox: u64,
        op: u16,
    ) -> Result<Self, Error> {
        let (header, body) = Header::decode(&reply.payload)?;
        if header.op != op {
            return Err(Error::Invalid);
        }
        if let Some(error) = header.error {
            if !body.is_empty() || reply.handles.remaining() != 0 {
                return Err(Error::Invalid);
            }
            return Err(error);
        }
        let mut reader = librpc::bytes::Reader::new(body);
        let snapshot = Snapshot::decode(reader.bytes(Snapshot::BYTES)?)?;
        let function_name = reader.text32()?;
        let description_bytes = reader.bytes32()?;
        reader.finish()?;
        if (op != 2 || snapshot.phase != Phase::Ready)
            && (!description_bytes.is_empty() || !function_name.is_empty())
        {
            return Err(Error::Invalid);
        }
        if op == 2 && snapshot.phase == Phase::Ready {
            liboal::path::validate_name(function_name).map_err(|_| Error::Invalid)?;
        }
        let mut name = String::new();
        name.try_reserve_exact(function_name.len())
            .map_err(|_| Error::Resource)?;
        name.push_str(function_name);
        let mut description = Vec::new();
        description
            .try_reserve_exact(description_bytes.len())
            .map_err(|_| Error::Resource)?;
        description.extend_from_slice(description_bytes);
        if snapshot.reference != expected {
            return Err(Error::StaleEpoch);
        }
        let expected_handles =
            usize::from(op == 1) + usize::from(op == 2 && snapshot.phase == Phase::Ready);
        if reply.handles.remaining() != expected_handles {
            return Err(Error::Invalid);
        }
        let mut entry = |index| -> Result<MailboxSender, Error> {
            let (entry, description) = MailboxSender::from_capability(reply.handles.take(index)?)
                .map_err(|failure| Error::from(failure.error))?;
            if description.related_object_id != worker_mailbox {
                return Err(Error::Permission);
            }
            Ok(entry)
        };
        let observation = if op == 1 { Some(entry(0)?) } else { None };
        let function = if op == 2 && snapshot.phase == Phase::Ready {
            Some(entry(0)?)
        } else {
            None
        };
        Ok(Self {
            snapshot,
            observation,
            function,
            description,
            name,
        })
    }
}
pub fn decode_snapshot(
    reply: &librpc::Reply,
    op: u16,
    handles: Option<usize>,
) -> Result<Snapshot, Error> {
    let (header, body) = Header::decode(&reply.payload)?;
    if header.op != op {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() || reply.handles.remaining() != 0 {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    if handles.is_some_and(|expected| reply.handles.remaining() != expected) {
        return Err(Error::Invalid);
    }
    Snapshot::decode(body)
}
pub struct Watch {
    control: MailboxSender,
    observer: NotificationObserver,
    snapshot: Snapshot,
}
impl Watch {
    pub fn from_reply(
        control: MailboxSender,
        mut reply: librpc::Reply,
        reference: libdevice::identity::BindingRef,
    ) -> Result<Self, Error> {
        let snapshot = decode_snapshot(&reply, Observe::Watch.op(), Some(2))?;
        if snapshot.reference != reference {
            return Err(Error::StaleEpoch);
        }
        let root = control.description()?;
        let (control, description) =
            MailboxSender::from_capability(reply.handles.take(0)?).map_err(|f| f.error)?;
        if description.related_object_id != root.related_object_id
            || description.object_id == root.object_id
            || !description.rights.contains(crate::worker::TRANSPORT)
        {
            return Err(Error::Permission);
        }
        let (observer, description) = NotificationObserver::from_capability(reply.handles.take(1)?)
            .map_err(|failure| Error::from(failure.error))?;
        if !description
            .rights
            .contains(Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)
        {
            return Err(Error::Permission);
        }
        Ok(Self {
            control,
            observer,
            snapshot,
        })
    }
    pub fn control(&self) -> &MailboxSender {
        &self.control
    }
    pub fn observer(&self) -> &NotificationObserver {
        &self.observer
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }
}
impl Protocol for Watch {
    type Snapshot = Snapshot;
    type Status = Error;
    fn into_parts(self) -> (MailboxSender, NotificationObserver, Snapshot) {
        (self.control, self.observer, self.snapshot)
    }
    fn request(
        command: librpc::subscription::Command,
        deadline: Deadline,
    ) -> Result<librpc::Request, Failure<Error>> {
        let command = match command {
            librpc::subscription::Command::Query => Observe::Query,
            librpc::subscription::Command::Ack(sequence) => Observe::Ack(sequence),
        };
        let mut bytes = [0; protocol::HEADER_LEN + 8];
        let used = command
            .encode(deadline, &mut bytes)
            .map_err(Failure::Status)?;
        librpc::Request::new(protocol::OBSERVATION_ID, &bytes[..used]).map_err(Failure::System)
    }
    fn response(
        reply: &librpc::Reply,
        command: librpc::subscription::Command,
        _: Deadline,
    ) -> Result<Snapshot, Failure<Error>> {
        let op = match command {
            librpc::subscription::Command::Query => Observe::Query.op(),
            librpc::subscription::Command::Ack(_) => Observe::Ack(1).op(),
        };
        decode_snapshot(reply, op, Some(0)).map_err(Failure::Status)
    }
    fn observation(snapshot: &Snapshot) -> SnapshotState {
        SnapshotState {
            sequence: snapshot.sequence,
            pending: snapshot.pending,
            terminal: snapshot.phase == Phase::Terminal,
        }
    }
}
