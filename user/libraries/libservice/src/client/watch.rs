//! 领域 Watch 与通用 RPC 订阅会话之间的协议绑定。
use super::{ClientError, Watch, rpc_request, slots};
use crate::protocol::{self, Request, Response, Status, WatchInfo, WatchState};
use erhino_shared::time::Deadline;
use libkernel::ipc::{message::MailboxSender, notification::NotificationObserver};
use librpc::subscription::{Command, Failure, Protocol, SnapshotState};

fn request(command: Command) -> Request<'static> {
    match command {
        Command::Query => Request::QueryWatch,
        Command::Ack(sequence) => Request::AckWatch { sequence },
    }
}

impl Protocol for Watch {
    type Snapshot = WatchInfo;
    type Status = Status;

    fn into_parts(self) -> (MailboxSender, NotificationObserver, WatchInfo) {
        (self.control.sender, self.control.waiter, self.info)
    }

    fn request(command: Command, deadline: Deadline) -> Result<librpc::Request, Failure<Status>> {
        rpc_request(&request(command), deadline, None).map_err(|error| match error {
            ClientError::System(error) => Failure::System(error),
            ClientError::Status(status) => Failure::Status(status),
            _ => Failure::Protocol,
        })
    }

    fn response(
        reply: &librpc::Reply,
        command: Command,
        deadline: Deadline,
    ) -> Result<WatchInfo, Failure<Status>> {
        match protocol::decode_response(
            &reply.payload,
            &request(command),
            deadline,
            slots(&reply.handles),
        ) {
            Ok(Response::Watch(info)) => Ok(info),
            Ok(Response::Error(status)) => Err(Failure::Status(status)),
            _ => Err(Failure::Protocol),
        }
    }

    fn observation(info: &WatchInfo) -> SnapshotState {
        SnapshotState {
            sequence: info.sequence,
            pending: info.pending != 0,
            terminal: info.state == WatchState::Terminal,
        }
    }
}
