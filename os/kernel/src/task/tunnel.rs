//! Tunnel 对象：Connection 持帧，Endpoint 持本地映射 lease，Invitation
//! 是一次性可转移授权。不存在全局 id 或 registry。

use alloc::{sync::{Arc, Weak}, vec::Vec};
use core::{any::Any, sync::atomic::{AtomicBool, Ordering}};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
};

use crate::{
    frame::{self, FrameTracker},
    sync::Spinlock,
    task::{
        handle,
        object::{
            HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
            SubscribeResult,
        },
        proc::Process,
        wait::{finish_offered, Subscription},
        Thread,
    },
};

enum SideState {
    Alive(Weak<Endpoint>),
    Invited(Weak<Invitation>),
    Closed,
}

struct ConnectionState {
    pa: usize,
    #[expect(dead_code, reason = "Connection 的所有权字段，Drop 即归还共享帧")]
    frame: FrameTracker,
    sides: [SideState; 2],
}

struct Connection {
    state: Spinlock<ConnectionState>,
}

enum PeerNotice {
    Endpoint(Weak<Endpoint>),
    Invitation(Weak<Invitation>),
}

pub struct Endpoint {
    #[expect(dead_code, reason = "KernelObject 共同头供后续对象诊断使用")]
    header: ObjectHeader,
    connection: Arc<Connection>,
    side: usize,
    va: usize,
    closed: AtomicBool,
    wait: Spinlock<ObjectWaitState>,
}

impl Endpoint {
    fn new(connection: Arc<Connection>, side: usize, va: usize) -> Arc<Self> {
        Arc::new(Self {
            header: ObjectHeader::new(),
            connection,
            side,
            va,
            closed: AtomicBool::new(false),
            wait: Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
    }

    fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn set_signals(&self, signals: ObjectSignals) {
        self.wait.lock().update(ObjectSignals::NONE, signals);
        self.finish_waiters();
    }

    fn acknowledge_data(&self) {
        self.wait.lock().update(ObjectSignals::DATA, ObjectSignals::NONE);
    }

    fn finish_waiters(&self) {
        loop {
            let context = self.wait.lock().take_completer();
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }

    fn close(&self, owner: &Process) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.wait
            .lock()
            .update(ObjectSignals::DATA, ObjectSignals::CLOSED);

        let notice = {
            let mut connection = self.connection.state.lock();
            let mine = &connection.sides[self.side];
            if !matches!(mine, SideState::Alive(endpoint) if endpoint.as_ptr() == self as *const Endpoint)
            {
                None
            } else {
                connection.sides[self.side] = SideState::Closed;
                let peer = 1 - self.side;
                match core::mem::replace(&mut connection.sides[peer], SideState::Closed) {
                    SideState::Alive(endpoint) => {
                        connection.sides[peer] = SideState::Alive(endpoint.clone());
                        Some(PeerNotice::Endpoint(endpoint))
                    }
                    SideState::Invited(invitation) => Some(PeerNotice::Invitation(invitation)),
                    SideState::Closed => None,
                }
            }
        };

        owner.space.lock().unmap_external(self.va);
        self.finish_waiters();
        match notice {
            Some(PeerNotice::Endpoint(endpoint)) => {
                if let Some(endpoint) = endpoint.upgrade() {
                    endpoint.set_signals(ObjectSignals::PEER_CLOSED);
                }
            }
            Some(PeerNotice::Invitation(invitation)) => {
                if let Some(invitation) = invitation.upgrade() {
                    invitation.mark_closed();
                }
            }
            None => {}
        }
    }

    fn notify_peer(&self) -> Result<(), SystemCallError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SystemCallError::ObjectClosed);
        }
        let peer = {
            let connection = self.connection.state.lock();
            match &connection.sides[1 - self.side] {
                SideState::Alive(endpoint) => endpoint.clone(),
                SideState::Invited(_) => return Err(SystemCallError::ObjectNotAvailable),
                SideState::Closed => return Err(SystemCallError::ObjectClosed),
            }
        };
        let peer = peer.upgrade().ok_or(SystemCallError::ObjectClosed)?;
        peer.set_signals(ObjectSignals::DATA);
        Ok(())
    }
}

impl KernelObject for Endpoint {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::TunnelEndpoint
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::TunnelEndpoint)
            .then_some(Rights::WAIT | Rights::SIGNAL | Rights::MANAGE)
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::TunnelEndpoint).then_some(
            ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
        )
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.wait.lock().unsubscribe(id);
    }

    fn close_handle(&self, role: HandleRole, owner: &Process, _exiting: bool) {
        debug_assert!(role == HandleRole::TunnelEndpoint);
        self.close(owner);
    }

    fn close_transit(&self, _role: HandleRole) {
        unreachable!("Tunnel Endpoint cannot enter transit")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct Invitation {
    #[expect(dead_code, reason = "KernelObject 共同头供后续对象诊断使用")]
    header: ObjectHeader,
    connection: Arc<Connection>,
    side: usize,
    closed: AtomicBool,
    wait: Spinlock<ObjectWaitState>,
}

impl Invitation {
    fn new(connection: Arc<Connection>, side: usize) -> Arc<Self> {
        Arc::new(Self {
            header: ObjectHeader::new(),
            connection,
            side,
            closed: AtomicBool::new(false),
            wait: Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
    }

    fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn mark_closed(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.wait
            .lock()
            .update(ObjectSignals::NONE, ObjectSignals::CLOSED);
    }

    fn abandon(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let creator = {
            let mut connection = self.connection.state.lock();
            if !matches!(
                &connection.sides[self.side],
                SideState::Invited(invitation) if invitation.as_ptr() == self as *const Invitation
            ) {
                None
            } else {
                connection.sides[self.side] = SideState::Closed;
                match &connection.sides[1 - self.side] {
                    SideState::Alive(endpoint) => Some(endpoint.clone()),
                    _ => None,
                }
            }
        };
        self.wait
            .lock()
            .update(ObjectSignals::NONE, ObjectSignals::CLOSED);
        if let Some(endpoint) = creator.and_then(|endpoint| endpoint.upgrade()) {
            endpoint.set_signals(ObjectSignals::PEER_CLOSED);
        }
    }
}

impl KernelObject for Invitation {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::TunnelInvitation
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::TunnelInvitation).then_some(Rights::MAP | Rights::TRANSFER)
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::TunnelInvitation).then_some(ObjectSignals::CLOSED)
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.wait.lock().unsubscribe(id);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        debug_assert!(role == HandleRole::TunnelInvitation);
        self.abandon();
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::TunnelInvitation);
        self.abandon();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(thread: &Thread, va: usize, output: usize) -> Result<(), SystemCallError> {
    let tracker = frame::alloc_contiguous(1).ok_or(SystemCallError::OutOfMemory)?;
    let pa = tracker.base.addr();
    let connection = Arc::new(Connection {
        state: Spinlock::new(ConnectionState {
            pa,
            frame: tracker,
            sides: [SideState::Closed, SideState::Closed],
        }),
    });
    let endpoint = Endpoint::new(connection.clone(), 0, va);
    let invitation = Invitation::new(connection.clone(), 1);
    {
        let mut state = connection.state.lock();
        state.sides = [
            SideState::Alive(Arc::downgrade(&endpoint)),
            SideState::Invited(Arc::downgrade(&invitation)),
        ];
    }

    let mut entries = Vec::new();
    entries.try_reserve_exact(2).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(
        handle::entry(
            Endpoint::object_ref(&endpoint),
            HandleRole::TunnelEndpoint,
            Rights::WAIT | Rights::SIGNAL | Rights::MANAGE,
        )
        .map_err(handle::map_error)?,
    );
    entries.push(
        handle::entry(
            Invitation::object_ref(&invitation),
            HandleRole::TunnelInvitation,
            Rights::MAP | Rights::TRANSFER,
        )
        .map_err(handle::map_error)?,
    );

    let token = handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(2, token).map_err(handle::map_error)?;
    let pair = HandlePair::new(reservation.handles()[0], reservation.handles()[1]);
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<HandlePair>(), true) {
        table.rollback(reservation).expect("TunnelCreate reservation must remain owned");
        return Err(error.into());
    }
    if let Err(error) = space.map_external(va, pa) {
        table.rollback(reservation).expect("TunnelCreate reservation must remain owned");
        return Err(map_space_error(error));
    }
    // SAFETY: HandlePair 无 padding，输出已在同一 space 锁下校验。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &pair) }
        .expect("validated TunnelCreate output must remain writable");
    table
        .commit(reservation, entries)
        .expect("TunnelCreate reservation must remain owned");
    Ok(())
}

pub fn attach(
    thread: &Thread,
    invitation_handle: Handle,
    va: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let token = handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let object = {
        let entry = table
            .get(invitation_handle, Rights::MAP)
            .map_err(handle::map_error)?;
        if *entry.role() != HandleRole::TunnelInvitation
            || entry.object().kind() != ObjectKind::TunnelInvitation
        {
            return Err(SystemCallError::WrongObjectType);
        }
        entry.object().clone()
    };
    let invitation = concrete_invitation(&object)?;
    let reservation = table.reserve(1, token).map_err(handle::map_error)?;
    let endpoint_handle = reservation.handles()[0];
    let endpoint = Endpoint::new(invitation.connection.clone(), invitation.side, va);
    let endpoint_entry = handle::entry(
        Endpoint::object_ref(&endpoint),
        HandleRole::TunnelEndpoint,
        Rights::WAIT | Rights::SIGNAL | Rights::MANAGE,
    )
    .map_err(handle::map_error)?;
    let mut entries = Vec::new();
    entries.try_reserve_exact(1).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(endpoint_entry);

    let mut connection = invitation.connection.state.lock();
    if invitation.closed.load(Ordering::Acquire)
        || !matches!(
            &connection.sides[invitation.side],
            SideState::Invited(candidate) if candidate.as_ptr() == invitation as *const Invitation
        )
        || !matches!(connection.sides[1 - invitation.side], SideState::Alive(_))
    {
        table.rollback(reservation).expect("TunnelAttach reservation must remain owned");
        return Err(SystemCallError::ObjectClosed);
    }
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        table.rollback(reservation).expect("TunnelAttach reservation must remain owned");
        return Err(error.into());
    }
    if let Err(error) = space.map_external(va, connection.pa) {
        table.rollback(reservation).expect("TunnelAttach reservation must remain owned");
        return Err(map_space_error(error));
    }

    connection.sides[invitation.side] = SideState::Alive(Arc::downgrade(&endpoint));
    invitation.closed.store(true, Ordering::Release);
    let consumed = table
        .remove(invitation_handle)
        .expect("validated Tunnel invitation must remain installed");
    table
        .commit(reservation, entries)
        .expect("TunnelAttach reservation must remain owned");
    // SAFETY: Handle 无 padding，输出已在同一 space 锁下校验。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &endpoint_handle) }
        .expect("validated TunnelAttach output must remain writable");
    drop(consumed); // invitation 是被消费而非关闭，不执行 lifecycle callback。
    Ok(())
}

pub fn notify(thread: &Thread, handle: Handle) -> Result<(), SystemCallError> {
    let object = resolve_endpoint(thread, handle, Rights::SIGNAL)?;
    concrete_endpoint(&object)?.notify_peer()
}

pub fn acknowledge_data(thread: &Thread, handle: Handle) -> Result<(), SystemCallError> {
    let object = resolve_endpoint(thread, handle, Rights::MANAGE)?;
    concrete_endpoint(&object)?.acknowledge_data();
    Ok(())
}

fn resolve_endpoint(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(handle, rights).map_err(handle::map_error)?;
    if *entry.role() != HandleRole::TunnelEndpoint
        || entry.object().kind() != ObjectKind::TunnelEndpoint
    {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

fn concrete_endpoint(object: &ObjectRef) -> Result<&Endpoint, SystemCallError> {
    object
        .as_any()
        .downcast_ref::<Endpoint>()
        .ok_or(SystemCallError::WrongObjectType)
}

fn concrete_invitation(object: &ObjectRef) -> Result<&Invitation, SystemCallError> {
    object
        .as_any()
        .downcast_ref::<Invitation>()
        .ok_or(SystemCallError::WrongObjectType)
}

fn map_space_error(error: super::proc::SpaceError) -> SystemCallError {
    match error {
        super::proc::SpaceError::BadSegment => SystemCallError::IllegalArgument,
        super::proc::SpaceError::NoFrame => SystemCallError::OutOfMemory,
        super::proc::SpaceError::Conflict => SystemCallError::InvalidAddress,
    }
}
