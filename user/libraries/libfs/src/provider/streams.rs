//! 原文件 Request 的会话建立与控制；业务 owner 留在唯一 StreamTable。
use super::*;
use crate::protocol::{Direction, StreamState};
impl Request {
    pub const MAX_SOURCES: usize = 3;
    pub(super) fn prepare_stream(
        &mut self,
        backend: &MemoryBackend,
        mailbox: &libkernel::ipc::message::Mailbox,
        host: &mut StreamHost<'_>,
    ) -> Result<(), Status> {
        if self.domain != Domain::Fs
            || self.header.version != 1
            || !matches!(
                protocol::Op::from_raw(self.header.op),
                Some(protocol::Op::Open | protocol::Op::CreateOpen)
            )
        {
            return Ok(());
        }
        let response = self
            .outbox
            .as_mut()
            .ok_or(Status::Internal)?
            .response_mut()
            .map_err(failure)?;
        let (context, _) = response.parts().map_err(failure)?;
        let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
        let access = self
            .access
            .as_ref()
            .ok_or(self.denied.unwrap_or(Status::Permission))?;
        let (target, options, creation, read_end) = match request {
            FsRequest::Open { path, options } => {
                let Resolved::Local(target) = backend.resolve(access, path, true)? else {
                    return Ok(());
                };
                let right = if options.direction == Direction::Read {
                    FsRights::READ_STREAM
                } else {
                    FsRights::WRITE_STREAM
                };
                backend.check(access, &target, right)?;
                let length = backend.stream_length(&target)?;
                let read_end = if options.direction == Direction::Read {
                    options
                        .length
                        .map_or(length, |n| options.offset + n)
                        .min(length)
                        .max(options.offset)
                } else {
                    0
                };
                (target, options, None, read_end)
            }
            FsRequest::CreateOpen {
                path,
                options,
                rights,
            } => {
                let (parent, name) =
                    backend.parent_for_operation(access, path, FsRights::CREATE)?;
                let Resolved::Local(parent) = parent else {
                    return Ok(());
                };
                if !access.rights().contains(FsRights::WRITE_STREAM)
                    || !rights.contains(FsRights::WRITE_STREAM)
                {
                    return Err(Status::Permission);
                }
                let creation = backend.prepare_create(
                    access,
                    parent,
                    name,
                    crate::node::NodeKind::Stream,
                    rights,
                    &[],
                )?;
                let target = creation.created_target().ok_or(Status::Internal)?;
                (target, options, Some(creation), 0)
            }
            _ => return Err(Status::Invalid),
        };
        let clock = libkernel::time::snapshot().map_err(failure)?;
        let mut stream = host.table.prepare(
            mailbox,
            access,
            target,
            options,
            self.header.deadline,
            clock,
            read_end,
            host.driver,
            host.pool,
            host.config,
        )?;
        let context = stream.context();
        let error = stream.error;
        self.stream_deadline = stream.offer.offer_deadline;
        self.life = Some(SubscriptionLife {
            kind: ControlKind::Stream,
            context,
            lifetime: stream.lifetime.take(),
            source: libexecution::SourceRegistration::new(),
            committed: false,
            closed: false,
            retry_at: None,
        });
        self.pending = Some(PreparedControl::Stream(PreparedFileStream {
            stream,
            creation,
        }));
        error.map_or(Ok(()), Err)
    }
    pub(super) fn stream_precommit<F>(
        &mut self,
        host: &mut StreamHost<'_>,
        requests: &mut Requests<'_, F>,
        now: u64,
    ) -> Result<bool, SystemCallError> {
        let Some(PreparedControl::Stream(pending)) = self.pending.as_mut() else {
            return Ok(true);
        };
        if self.data_source.is_empty() {
            let plan = pending
                .stream
                .terminal_plan()
                .expect("prepared stream lost mapped endpoint");
            if self
                .data_source
                .install(requests, plan, self.data_source_kind)
                .is_err()
            {
                return Ok(false);
            }
            self.data_source_active = false;
        }
        if self.data_source.source().is_none() {
            return Ok(false);
        }
        let status = if pending
            .stream
            .offer
            .offer_deadline
            .instant()
            .ok()
            .flatten()
            .is_some_and(|at| at <= now)
        {
            Some(Status::Expired)
        } else {
            host.table.can_install(&mut pending.stream).err()
        };
        if let Some(status) = status {
            self.abort = Some(status);
            return Ok(false);
        }
        Ok(true)
    }
    pub(super) fn commit_stream(
        &mut self,
        mut pending: PreparedFileStream,
        backend: &mut MemoryBackend,
        host: &mut StreamHost<'_>,
    ) -> Result<(), SystemCallError> {
        let response = self
            .outbox
            .as_mut()
            .expect("stream reply owner missing")
            .response_mut()?;
        let (_, out) = response.parts()?;
        let used = protocol::encode_response(
            self.header.op,
            &FsResponse::Offer(pending.stream.offer),
            self.header.deadline,
            out,
        )
        .map_err(|_| SystemCallError::IllegalArgument)?;
        // 固定两槽及完整运输 mask 已在准备时验证，能力归 Outbox 的实际释放链。
        let sender = pending
            .stream
            .sender
            .take()
            .expect("stream control sender missing");
        let invitation = pending
            .stream
            .invitation
            .take()
            .expect("stream invitation missing");
        response
            .push(sender.into_capability(), pending.stream.sender_rights)
            .map_err(|failed| failed.error)?;
        response
            .push(
                invitation.into_capability(),
                crate::stream::invitation_transport(),
            )
            .map_err(|failed| failed.error)?;
        response.finish_body(used)?;
        if let Some(creation) = pending.creation.take()
            && let Err(failed) = backend.commit(creation)
        {
            pending.creation = Some(failed.mutation);
            self.abort = Some(failed.status);
            self.pending = Some(PreparedControl::Stream(pending));
            return Ok(());
        }
        host.table.install(pending.stream);
        self.life
            .as_mut()
            .expect("stream publication Lifetime missing")
            .committed = true;
        self.encoded = true;
        Ok(())
    }
    pub(super) fn cleanup_prepared_stream<F>(
        &mut self,
        requests: &mut Requests<'_, F>,
        now: u64,
    ) -> Result<bool, SystemCallError> {
        if !matches!(self.pending, Some(PreparedControl::Stream(_))) {
            return Ok(true);
        }
        if !self.data_source.is_empty() {
            if self.data_source.remove(requests).is_err() {
                return Ok(false);
            }
            return Ok(false);
        }
        let Some(PreparedControl::Stream(pending)) = self.pending.as_mut() else {
            unreachable!();
        };
        match pending.stream.cleanup(now) {
            Ok(true) => Ok(true),
            Ok(false) | Err(_) => {
                self.stream_deadline = pending.stream.cleanup_deadline();
                Ok(false)
            }
        }
    }
    pub(super) fn serve_stream_control(
        &mut self,
        host: &mut StreamHost<'_>,
        now: u64,
    ) -> Result<bool, SystemCallError> {
        if self.domain != Domain::Fs
            || self.header.version != 1
            || !matches!(
                protocol::Op::from_raw(self.header.op),
                Some(
                    protocol::Op::Start
                        | protocol::Op::QueryStream
                        | protocol::Op::Finish
                        | protocol::Op::Cancel
                )
            )
        {
            return Ok(false);
        }
        let response = self
            .outbox
            .as_mut()
            .expect("stream control reply missing")
            .response_mut()?;
        let (context, out) = response.parts()?;
        let context_id = context.envelope.sender_context_id;
        let result = (|| {
            let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
            if self
                .header
                .deadline
                .instant()
                .ok()
                .flatten()
                .is_some_and(|at| at <= now)
            {
                return Err(Status::Expired);
            }
            match request {
                FsRequest::Start => host.table.start(context_id, now).map(Some),
                FsRequest::QueryStream => host.table.query(context_id).map(Some),
                FsRequest::Cancel => host.table.cancel(context_id).map(Some),
                FsRequest::Finish => {
                    host.table
                        .finish(context_id, host.driver, self.header.deadline, now)
                }
                _ => Err(Status::Invalid),
            }
        })();
        let reply = match result {
            Ok(Some(info)) => FsResponse::Stream(info),
            Ok(None) => {
                self.finish_context = Some(context_id);
                // 等待槽已接管业务责任；回复权继续在 Outbox，入口 use 不保活
                // 控制 Lifetime，否则最后控制关闭会与等待 Terminal 循环。
                if context.finish_delivery().is_err() {
                    self.stream_deadline =
                        Deadline::at(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
                }
                return Ok(true);
            }
            Err(status) => FsResponse::Error(status),
        };
        let used = protocol::encode_response(self.header.op, &reply, self.header.deadline, out)
            .map_err(|_| SystemCallError::IllegalArgument)?;
        response.finish_body(used)?;
        self.encoded = true;
        Ok(true)
    }
    pub(super) fn finish_wait(
        &mut self,
        table: &mut crate::stream::StreamTable,
        task: u64,
        now: u64,
    ) -> Result<bool, SystemCallError> {
        let Some(context) = self.finish_context else {
            return Ok(false);
        };
        if self.stopping
            || self
                .outbox
                .as_ref()
                .is_none_or(|outbox| outbox.result().is_some())
        {
            table.release_finish(context, task);
            self.finish_context = None;
            return Ok(false);
        }
        if self
            .stream_deadline
            .instant()
            .ok()
            .flatten()
            .is_some_and(|at| now < at)
        {
            return Ok(true);
        }
        let response = self
            .outbox
            .as_mut()
            .expect("Finish waiter lost reply")
            .response_mut()?;
        if response.parts()?.0.finish_delivery().is_err() {
            self.stream_deadline =
                Deadline::at(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
            return Ok(true);
        }
        self.stream_deadline = Deadline::INFINITE;
        let info = table
            .query(context)
            .map_err(|_| SystemCallError::ObjectClosed)?;
        if info.state != StreamState::Terminal {
            return Ok(true);
        }
        let response = self
            .outbox
            .as_mut()
            .expect("Finish waiter lost reply")
            .response_mut()?;
        let used = protocol::encode_response(
            self.header.op,
            &FsResponse::Stream(info),
            self.header.deadline,
            response.body_mut()?,
        )
        .map_err(|_| SystemCallError::IllegalArgument)?;
        response.finish_body(used)?;
        self.encoded = true;
        table.release_finish(context, task);
        self.finish_context = None;
        Ok(false)
    }
}
