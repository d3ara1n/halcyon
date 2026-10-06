//! 独立线程的 Pending 启动责任；观察 Control 不持有放弃权。

use alloc::sync::{Arc, Weak};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, ObjectSignals, Rights},
};

use crate::sync::{Spinlock, ranks};

use super::{
    super::object::{KernelObject, ObjectHeader, ObjectRef},
    thread::Thread,
};

enum TicketState {
    Prepared(Weak<Thread>),
    Pending(Weak<Thread>),
    Starting,
    Started,
    Aborted,
}

pub(crate) struct ThreadStartTicket {
    header: ObjectHeader,
    holders: capability::Holders,
    process_id: u64,
    state: Spinlock<TicketState>,
    _metadata: super::super::resources::MetadataCharge,
}

impl ThreadStartTicket {
    pub(crate) fn try_new(thread: Arc<Thread>) -> Result<Arc<Self>, SystemCallError> {
        let metadata =
            super::super::resources::MetadataCharge::arc::<Self>(&thread.process.funding_pool())?;
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            process_id: thread.process.header.koid(),
            // 真实执行容器由 Created 成员拥有；外域票据不能阻止 Frozen 回收私有现场。
            state: Spinlock::new(ranks::LEAF, TicketState::Prepared(Arc::downgrade(&thread))),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    /// 成员提交后、capability 表项公开前执行，不分配、不调用外部对象。
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock();
        match core::mem::replace(&mut *state, TicketState::Starting) {
            TicketState::Prepared(thread) => *state = TicketState::Pending(thread),
            _ => panic!("thread start ticket armed twice"),
        }
    }

    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        if object.kind() != ObjectKind::ThreadStartTicket {
            return Err(SystemCallError::WrongObjectType);
        }
        let erased: Arc<dyn Any + Send + Sync> = object.clone();
        erased
            .downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    pub(crate) fn start(&self) -> Result<(), SystemCallError> {
        let thread = {
            let mut state = self.state.lock();
            match core::mem::replace(&mut *state, TicketState::Starting) {
                TicketState::Pending(thread) => thread,
                previous => {
                    *state = previous;
                    return Err(SystemCallError::ObjectNotAvailable);
                }
            }
        };
        let result = thread
            .upgrade()
            .ok_or(SystemCallError::ObjectClosed)
            .and_then(|thread| {
                let result = thread.start();
                if result.is_err() {
                    thread.abandon_created();
                }
                result
            });
        match &result {
            Ok(()) => *self.state.lock() = TicketState::Started,
            Err(_) => {
                *self.state.lock() = TicketState::Aborted;
            }
        }
        result
    }
}

impl KernelObject for ThreadStartTicket {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn holders(&self) -> &capability::Holders {
        &self.holders
    }

    fn on_last_capability(&self) {
        let thread = {
            let mut state = self.state.lock();
            match core::mem::replace(&mut *state, TicketState::Aborted) {
                TicketState::Pending(thread) => Some(thread),
                TicketState::Prepared(thread) => {
                    drop(state);
                    drop(thread);
                    return;
                }
                previous => {
                    *state = previous;
                    None
                }
            }
        };
        if let Some(thread) = thread.and_then(|thread| thread.upgrade()) {
            thread.abandon_created();
        }
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::ThreadStartTicket
    }

    fn related_id(&self) -> u64 {
        self.process_id
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::MANAGE | Rights::GRANT | Rights::TRANSIT)
    }

    fn allowed_signals(&self) -> Option<ObjectSignals> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
