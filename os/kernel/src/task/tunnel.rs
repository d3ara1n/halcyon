//! Tunnel 对象：Connection 持帧，Endpoint 持本地映射 lease，Invitation
//! 是一次性可转移授权。不存在全局 id 或 registry。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
};
use memory_space::{
    BackingView, MemoryObjectState, ObjectError, ObjectId, Protection, RegionKindView, RegionOwner,
    RetiringFragment, WritePermit,
};

use crate::{
    frame::{self, FrameTracker},
    sync::Spinlock,
    task::{
        Thread, handle,
        object::{
            HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
            SubscribeResult,
        },
        proc::{
            AddressSpaceState, MemoryRetireSink, ObjectMappingLease, PrepareShootdownError,
            PreparedObjectMapping, Process, RetiringSpaceChange, prepare_memory_completion,
        },
        wait::{Subscription, finish_offered},
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
    leases: [Option<ObjectMappingLease>; 2],
    sides: [SideState; 2],
}

struct Connection {
    memory: Spinlock<MemoryObjectState>,
    state: Spinlock<ConnectionState>,
}

static NEXT_MEMORY_OBJECT: AtomicU64 = AtomicU64::new(1);

fn mint_memory_object() -> ObjectId {
    let identity = NEXT_MEMORY_OBJECT.fetch_add(1, Ordering::Relaxed);
    ObjectId::new(identity).expect("Tunnel MemoryObject identity exhausted")
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
    closed: AtomicBool,
    wait: Spinlock<ObjectWaitState>,
    // 在途时形成 Endpoint → LeaseRetire → Endpoint 的临时强环；pending_close
    // 持 entry 并逐批重入，完成分支先 take 本字段再 drop entry，从而打破环。
    // 任何新增的放弃 entry 路径都必须先显式拆除此状态。
    detached_retire: Spinlock<Option<DetachedLeaseRetire>>,
}

impl Endpoint {
    fn new(connection: Arc<Connection>, side: usize) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            connection,
            side,
            closed: AtomicBool::new(false),
            wait: Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
            detached_retire: Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn set_signals(&self, signals: ObjectSignals) {
        self.wait.lock().update(ObjectSignals::NONE, signals);
        self.finish_waiters();
    }

    fn acknowledge_data(&self) {
        self.wait
            .lock()
            .update(ObjectSignals::DATA, ObjectSignals::NONE);
    }

    fn finish_waiters(&self) {
        loop {
            let context = self.wait.lock().take_completer();
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }

    fn finish_close(&self, notice: Option<PeerNotice>) {
        self.wait
            .lock()
            .update(ObjectSignals::DATA, ObjectSignals::CLOSED);
        self.finish_waiters();
        publish_peer_notice(notice);
    }

    fn notify_peer(&self) -> Result<(), SystemCallError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SystemCallError::ObjectClosed);
        }
        let peer = {
            let connection = self.connection.state.lock();
            if !matches!(
                &connection.sides[self.side],
                SideState::Alive(endpoint) if endpoint.as_ptr() == self as *const Endpoint
            ) {
                return Err(SystemCallError::ObjectClosed);
            }
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

fn publish_peer_notice(notice: Option<PeerNotice>) {
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
        (role == HandleRole::TunnelEndpoint)
            .then_some(ObjectSignals::DATA | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED)
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
        debug_assert!(role == HandleRole::TunnelEndpoint);
        unreachable!("Tunnel Endpoint close must consume its mapping lease transaction")
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
}

impl Invitation {
    fn new(connection: Arc<Connection>, side: usize) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            connection,
            side,
            closed: AtomicBool::new(false),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
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
        (role == HandleRole::TunnelInvitation)
            .then_some(Rights::MAP | Rights::TRANSIT | Rights::GRANT)
    }

    fn allowed_signals(&self, _role: HandleRole) -> Option<ObjectSignals> {
        None
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

fn map_object_error(error: ObjectError) -> SystemCallError {
    match error {
        ObjectError::AllocationFailed => SystemCallError::OutOfMemory,
        ObjectError::PermitLimit => SystemCallError::ReachLimit,
        ObjectError::ViewDenied | ObjectError::PermitDenied | ObjectError::Busy => {
            SystemCallError::ObjectBusy
        }
        ObjectError::PermitOverflow | ObjectError::InvalidWaiter => SystemCallError::InternalError,
    }
}

fn map_shootdown_error(error: PrepareShootdownError) -> SystemCallError {
    match error {
        PrepareShootdownError::NotRunning => SystemCallError::ObjectClosed,
        PrepareShootdownError::Busy => SystemCallError::ObjectBusy,
        PrepareShootdownError::InvalidTargets => SystemCallError::InternalError,
        PrepareShootdownError::OutOfMemory => SystemCallError::OutOfMemory,
    }
}

fn reserve_mapping(
    connection: &Connection,
) -> Result<
    (
        memory_space::ObjectViewAuthorization,
        Vec<memory_space::WritePermit>,
    ),
    SystemCallError,
> {
    let mut memory = connection.memory.lock();
    let authorization = memory
        .authorize_view(Protection::ReadWrite)
        .map_err(map_object_error)?;
    let permits = memory.reserve_writes(1).map_err(map_object_error)?;
    Ok((authorization, permits))
}

fn cancel_writes(connection: &Connection, permits: Vec<memory_space::WritePermit>) {
    let waiter = connection.memory.lock().cancel_writes(permits);
    assert!(
        waiter.is_none(),
        "Tunnel MemoryObject cannot have a seal waiter"
    );
}

fn prepare_mapping(
    connection: &ConnectionState,
    space: &mut AddressSpaceState,
    va: usize,
    authorization: memory_space::ObjectViewAuthorization,
    permits: Vec<memory_space::WritePermit>,
) -> Result<super::proc::ObjectMappingPlan, super::proc::ObjectMapFailure> {
    space.prepare_object_mapping(va, connection.pa, authorization, permits)
}

fn rollback_mapping(
    space: &mut AddressSpaceState,
    prepared: PreparedObjectMapping,
) -> (
    Vec<memory_space::WritePermit>,
    page_table::PreparedTranslation<super::proc::TableFrameToken>,
) {
    space.rollback_object_mapping(prepared)
}

fn install_mapping(connection: &mut ConnectionState, side: usize, lease: ObjectMappingLease) {
    let previous = connection.leases[side].replace(lease);
    assert!(
        previous.is_none(),
        "Tunnel side mapping lease installed twice"
    );
}

fn commit_side_close(endpoint: &Endpoint, connection: &mut ConnectionState) -> Option<PeerNotice> {
    assert!(
        matches!(
            &connection.sides[endpoint.side],
            SideState::Alive(candidate) if candidate.as_ptr() == endpoint as *const Endpoint
        ),
        "Tunnel close lost its live side"
    );
    connection.sides[endpoint.side] = SideState::Closed;
    endpoint.closed.store(true, Ordering::Release);
    let peer = 1 - endpoint.side;
    match core::mem::replace(&mut connection.sides[peer], SideState::Closed) {
        SideState::Alive(peer_endpoint) => {
            connection.sides[peer] = SideState::Alive(peer_endpoint.clone());
            Some(PeerNotice::Endpoint(peer_endpoint))
        }
        SideState::Invited(invitation) => Some(PeerNotice::Invitation(invitation)),
        SideState::Closed => None,
    }
}

fn validate_retired_lease_fragment(lease: ObjectMappingLease, fragment: RetiringFragment) {
    assert!(
        fragment.range == lease.range
            && fragment.owner == RegionOwner::Lease(lease.lease)
            && matches!(
                fragment.kind,
                RegionKindView::Mapping {
                    backing: BackingView::Object { object, offset: 0 },
                    current: Protection::ReadWrite,
                    maximum: Protection::ReadWrite,
                    ..
                } if object == lease.object
            ),
        "Tunnel retire fragment does not match its lease"
    );
}

struct LeaseRetireState {
    notice: Option<Option<PeerNotice>>,
    fragment_retired: bool,
    permit_retired: bool,
}

struct LeaseRetire {
    connection: Arc<Connection>,
    endpoint: Arc<Endpoint>,
    lease: ObjectMappingLease,
    state: Spinlock<LeaseRetireState>,
}

struct DetachedLeaseRetire {
    change: RetiringSpaceChange,
    sink: Arc<LeaseRetire>,
}

impl LeaseRetire {
    fn new(
        connection: Arc<Connection>,
        endpoint: Arc<Endpoint>,
        lease: ObjectMappingLease,
    ) -> Self {
        Self {
            connection,
            endpoint,
            lease,
            state: Spinlock::new(
                crate::sync::ranks::MEMORY_COMPLETION,
                LeaseRetireState {
                    notice: None,
                    fragment_retired: false,
                    permit_retired: false,
                },
            ),
        }
    }

    fn install_notice(&self, notice: Option<PeerNotice>) {
        let previous = self.state.lock().notice.replace(notice);
        assert!(previous.is_none(), "Tunnel close notice installed twice");
    }
}

impl MemoryRetireSink for LeaseRetire {
    fn retire_fragment(&self, fragment: RetiringFragment) {
        validate_retired_lease_fragment(self.lease, fragment);
        let mut state = self.state.lock();
        assert!(
            !state.fragment_retired,
            "Tunnel lease fragment retired twice"
        );
        state.fragment_retired = true;
    }

    fn retire_permit(&self, permit: WritePermit) {
        let waiter = self.connection.memory.lock().retire_write(permit);
        assert!(
            waiter.is_none(),
            "Tunnel MemoryObject cannot have a seal waiter"
        );
        let mut state = self.state.lock();
        assert!(!state.permit_retired, "Tunnel lease permit retired twice");
        state.permit_retired = true;
    }

    fn finish(&self) {
        let mut state = self.state.lock();
        assert!(
            state.fragment_retired && state.permit_retired,
            "Tunnel lease completed before all retire owners"
        );
        let notice = state
            .notice
            .take()
            .expect("Tunnel close retired before notice Commit");
        drop(state);
        self.endpoint.finish_close(notice);
    }
}

pub fn create(
    thread: &Thread,
    va: usize,
    output: usize,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let tracker = frame::alloc_user_order(0).ok_or(SystemCallError::OutOfMemory)?;
    let pa = tracker.base().addr();
    let connection = Arc::try_new(Connection {
        memory: Spinlock::new(
            crate::sync::ranks::MEMORY_OBJECT,
            MemoryObjectState::new(mint_memory_object(), 2),
        ),
        state: Spinlock::new(
            crate::sync::ranks::CONNECTION,
            ConnectionState {
                pa,
                frame: tracker,
                leases: [None, None],
                sides: [SideState::Closed, SideState::Closed],
            },
        ),
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let endpoint = Endpoint::new(connection.clone(), 0)?;
    let invitation = Invitation::new(connection.clone(), 1)?;

    let mut entries = Vec::new();
    entries
        .try_reserve_exact(2)
        .map_err(|_| SystemCallError::OutOfMemory)?;
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
            Rights::MAP | Rights::TRANSIT | Rights::GRANT,
        )
        .map_err(handle::map_error)?,
    );

    let token = handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let mut reservation = Some(table.reserve(2, token).map_err(handle::map_error)?);
    let pair = {
        let handles = reservation
            .as_ref()
            .expect("TunnelCreate reservation exists")
            .handles();
        HandlePair::new(handles[0], handles[1])
    };
    {
        let mut space = thread.process.space.lock();
        if let Err(error) = space.check_range(output, core::mem::size_of::<HandlePair>(), true) {
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error.into());
        }
    }
    let mut connection_state = connection.state.lock();
    let (authorization, permits) = match reserve_mapping(&connection) {
        Ok(reserved) => reserved,
        Err(error) => {
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error);
        }
    };
    let mut mapping = {
        let (plan, pool) = {
            let mut space = thread.process.space.lock();
            match prepare_mapping(&connection_state, &mut space, va, authorization, permits) {
                Ok(plan) => (plan, Arc::clone(space.pool())),
                Err(failure) => {
                    drop(space);
                    cancel_writes(&connection, failure.permits);
                    table
                        .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                        .expect("TunnelCreate reservation must remain owned");
                    return Err(map_space_error(failure.error));
                }
            }
        };
        let owners = match super::proc::supply_funded_table_frames(&pool, plan.table_budget()) {
            Ok(owners) => owners,
            Err(error) => {
                let permits = thread
                    .process
                    .space
                    .lock()
                    .rollback_object_mapping_plan(plan);
                cancel_writes(&connection, permits);
                table
                    .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                    .expect("TunnelCreate reservation must remain owned");
                return Err(map_space_error(error));
            }
        };
        let mut space = thread.process.space.lock();
        match space.complete_object_mapping(plan, owners) {
            Ok(prepared) => Some(prepared),
            Err((failure, reclaimed)) => {
                drop(space);
                cancel_writes(&connection, failure.permits);
                drop(reclaimed);
                table
                    .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                    .expect("TunnelCreate reservation must remain owned");
                return Err(map_space_error(failure.error));
            }
        }
    };

    let (completion, plan) = match prepare_memory_completion(thread.process.clone(), 0, None, None)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping.take().expect("TunnelCreate mapping exists"),
                )
            };
            drop(translation);
            cancel_writes(&connection, permits);
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error);
        }
    };
    let sink: Arc<dyn crate::remote_call::Completion> = completion.clone();
    let shootdown = match thread
        .process
        .space
        .prepare_shootdown(&thread.process.lifecycle, sink)
    {
        Ok(shootdown) => shootdown,
        Err(error) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping.take().expect("TunnelCreate mapping exists"),
                )
            };
            drop(translation);
            cancel_writes(&connection, permits);
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(map_shootdown_error(error));
        }
    };

    {
        let mut space = thread.process.space.lock();
        // SAFETY: HandlePair 无 padding；复检失败即杀本进程。Commit 尚未发生，
        // 因而失败路径仍可完整回滚 handle、permit、ledger 与 PTE reservation。
        if let Err(error) =
            unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &pair) }
        {
            let (permits, translation) = rollback_mapping(
                &mut space,
                mapping.take().expect("TunnelCreate mapping exists"),
            );
            drop(space);
            drop(translation);
            cancel_writes(&connection, permits);
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error);
        }
    }

    let committed = thread.process.space.commit_shootdown(
        &thread.process.lifecycle,
        shootdown,
        va / super::proc::PAGE_SIZE,
        1,
        false,
        true,
        |space| {
            let (published, lease) = space.commit_object_mapping(
                mapping
                    .take()
                    .expect("TunnelCreate mapping commits exactly once"),
            );
            install_mapping(&mut connection_state, 0, lease);
            connection_state.sides = [
                SideState::Alive(Arc::downgrade(&endpoint)),
                SideState::Invited(Arc::downgrade(&invitation)),
            ];
            table
                .commit(
                    reservation.take().expect("TunnelCreate reservation exists"),
                    core::mem::take(&mut entries),
                )
                .expect("TunnelCreate reservation must remain owned");
            published
        },
    );
    let (published, synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping
                        .take()
                        .expect("stale TunnelCreate mapping must roll back"),
                )
            };
            drop(translation);
            cancel_writes(&connection, permits);
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(SystemCallError::ObjectBusy);
        }
    };
    drop(connection_state);
    drop(table);
    completion.install(published);
    synchronization.start();
    Ok(plan)
}

pub fn attach(
    thread: &Thread,
    invitation_handle: Handle,
    va: usize,
    output: usize,
) -> Result<super::wait::WaitPlan, SystemCallError> {
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
    let endpoint = Endpoint::new(invitation.connection.clone(), invitation.side)?;
    let endpoint_entry = handle::entry(
        Endpoint::object_ref(&endpoint),
        HandleRole::TunnelEndpoint,
        Rights::WAIT | Rights::SIGNAL | Rights::MANAGE,
    )
    .map_err(handle::map_error)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(endpoint_entry);
    let mut reservation = Some(table.reserve(1, token).map_err(handle::map_error)?);
    let endpoint_handle = reservation
        .as_ref()
        .expect("TunnelAttach reservation exists")
        .handles()[0];

    {
        let mut space = thread.process.space.lock();
        if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error.into());
        }
    }
    let mut connection_state = invitation.connection.state.lock();
    if invitation.closed.load(Ordering::Acquire)
        || !matches!(
            &connection_state.sides[invitation.side],
            SideState::Invited(candidate) if candidate.as_ptr() == invitation as *const Invitation
        )
        || !matches!(
            connection_state.sides[1 - invitation.side],
            SideState::Alive(_)
        )
        || connection_state.leases[invitation.side].is_some()
    {
        table
            .rollback(reservation.take().expect("TunnelAttach reservation exists"))
            .expect("TunnelAttach reservation must remain owned");
        return Err(SystemCallError::ObjectClosed);
    }
    let (authorization, permits) = match reserve_mapping(&invitation.connection) {
        Ok(reserved) => reserved,
        Err(error) => {
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error);
        }
    };
    let mut mapping = {
        let (plan, pool) = {
            let mut space = thread.process.space.lock();
            match prepare_mapping(&connection_state, &mut space, va, authorization, permits) {
                Ok(plan) => (plan, Arc::clone(space.pool())),
                Err(failure) => {
                    drop(space);
                    cancel_writes(&invitation.connection, failure.permits);
                    table
                        .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                        .expect("TunnelAttach reservation must remain owned");
                    return Err(map_space_error(failure.error));
                }
            }
        };
        let owners = match super::proc::supply_funded_table_frames(&pool, plan.table_budget()) {
            Ok(owners) => owners,
            Err(error) => {
                let permits = thread
                    .process
                    .space
                    .lock()
                    .rollback_object_mapping_plan(plan);
                cancel_writes(&invitation.connection, permits);
                table
                    .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                    .expect("TunnelAttach reservation must remain owned");
                return Err(map_space_error(error));
            }
        };
        let mut space = thread.process.space.lock();
        match space.complete_object_mapping(plan, owners) {
            Ok(prepared) => Some(prepared),
            Err((failure, reclaimed)) => {
                drop(space);
                cancel_writes(&invitation.connection, failure.permits);
                drop(reclaimed);
                table
                    .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                    .expect("TunnelAttach reservation must remain owned");
                return Err(map_space_error(failure.error));
            }
        }
    };

    let (completion, plan) = match prepare_memory_completion(thread.process.clone(), 0, None, None)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping.take().expect("TunnelAttach mapping exists"),
                )
            };
            drop(translation);
            cancel_writes(&invitation.connection, permits);
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error);
        }
    };
    let sink: Arc<dyn crate::remote_call::Completion> = completion.clone();
    let shootdown = match thread
        .process
        .space
        .prepare_shootdown(&thread.process.lifecycle, sink)
    {
        Ok(shootdown) => shootdown,
        Err(error) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping.take().expect("TunnelAttach mapping exists"),
                )
            };
            drop(translation);
            cancel_writes(&invitation.connection, permits);
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(map_shootdown_error(error));
        }
    };

    {
        let mut space = thread.process.space.lock();
        // SAFETY: Handle 无 padding；Commit 前复检失败按 fault 终止调用进程。
        if let Err(error) =
            unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &endpoint_handle) }
        {
            let (permits, translation) = rollback_mapping(
                &mut space,
                mapping.take().expect("TunnelAttach mapping exists"),
            );
            drop(space);
            drop(translation);
            cancel_writes(&invitation.connection, permits);
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error);
        }
    }

    let committed = thread.process.space.commit_shootdown(
        &thread.process.lifecycle,
        shootdown,
        va / super::proc::PAGE_SIZE,
        1,
        false,
        true,
        |space| {
            let consumed = table
                .remove(invitation_handle)
                .expect("TunnelAttach invitation is pinned by the table lock");
            let (published, lease) = space.commit_object_mapping(
                mapping
                    .take()
                    .expect("TunnelAttach mapping commits exactly once"),
            );
            install_mapping(&mut connection_state, invitation.side, lease);
            connection_state.sides[invitation.side] = SideState::Alive(Arc::downgrade(&endpoint));
            invitation.closed.store(true, Ordering::Release);
            table
                .commit(
                    reservation.take().expect("TunnelAttach reservation exists"),
                    core::mem::take(&mut entries),
                )
                .expect("TunnelAttach reservation must remain owned");
            (consumed, published)
        },
    );
    let ((consumed, published), synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            let (permits, translation) = {
                let mut space = thread.process.space.lock();
                rollback_mapping(
                    &mut space,
                    mapping
                        .take()
                        .expect("stale TunnelAttach mapping must roll back"),
                )
            };
            drop(translation);
            cancel_writes(&invitation.connection, permits);
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(SystemCallError::ObjectBusy);
        }
    };
    drop(connection_state);
    drop(table);
    drop(consumed); // invitation 被消费而非关闭，不执行 lifecycle callback。
    completion.install(published);
    synchronization.start();
    Ok(plan)
}

pub(crate) fn close_handle(
    thread: &Thread,
    handle: Handle,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let mut table = thread.process.handles.lock();
    let object = {
        let entry = table.get(handle, Rights::NONE).map_err(handle::map_error)?;
        if *entry.role() != HandleRole::TunnelEndpoint
            || entry.object().kind() != ObjectKind::TunnelEndpoint
        {
            return Err(SystemCallError::WrongObjectType);
        }
        entry.object().clone()
    };
    let endpoint = concrete_endpoint_arc(&object)?;
    let mut connection_state = endpoint.connection.state.lock();
    if endpoint.closed.load(Ordering::Acquire)
        || !matches!(
            &connection_state.sides[endpoint.side],
            SideState::Alive(candidate) if candidate.as_ptr() == Arc::as_ptr(&endpoint)
        )
    {
        return Err(SystemCallError::ObjectClosed);
    }
    let lease = connection_state.leases[endpoint.side].ok_or(SystemCallError::ObjectClosed)?;
    let mut unmap = {
        let (plan, pool) = {
            let mut space = thread.process.space.lock();
            let plan = space.prepare_object_unmap(lease).map_err(map_space_error)?;
            let pool = Arc::clone(space.pool());
            (plan, pool)
        };
        let owners = match super::proc::supply_funded_table_frames(&pool, plan.table_budget()) {
            Ok(owners) => owners,
            Err(error) => {
                thread.process.space.lock().rollback_object_unmap_plan(plan);
                return Err(map_space_error(error));
            }
        };
        let prepared = {
            let result = thread
                .process
                .space
                .lock()
                .complete_object_unmap(plan, owners);
            match result {
                Ok(prepared) => prepared,
                Err((error, owners)) => {
                    drop(owners);
                    return Err(map_space_error(error));
                }
            }
        };
        Some(prepared)
    };
    let retire = match Arc::try_new(LeaseRetire::new(
        endpoint.connection.clone(),
        endpoint.clone(),
        lease,
    )) {
        Ok(retire) => retire,
        Err(_) => {
            let translation = thread
                .process
                .space
                .lock()
                .rollback_object_unmap(unmap.take().expect("Tunnel close Unmap exists"));
            drop(translation);
            return Err(SystemCallError::OutOfMemory);
        }
    };
    let retire_sink: Arc<dyn MemoryRetireSink> = retire.clone();
    let (completion, plan) =
        match prepare_memory_completion(thread.process.clone(), 0, Some(retire_sink), None) {
            Ok(prepared) => prepared,
            Err(error) => {
                let translation = thread
                    .process
                    .space
                    .lock()
                    .rollback_object_unmap(unmap.take().expect("Tunnel close Unmap exists"));
                drop(translation);
                return Err(error);
            }
        };
    let sink: Arc<dyn crate::remote_call::Completion> = completion.clone();
    let shootdown = match thread
        .process
        .space
        .prepare_shootdown(&thread.process.lifecycle, sink)
    {
        Ok(shootdown) => shootdown,
        Err(error) => {
            let translation = thread
                .process
                .space
                .lock()
                .rollback_object_unmap(unmap.take().expect("Tunnel close Unmap exists"));
            drop(translation);
            return Err(map_shootdown_error(error));
        }
    };

    let committed = thread.process.space.commit_shootdown(
        &thread.process.lifecycle,
        shootdown,
        lease.range.start() / super::proc::PAGE_SIZE,
        lease.range.pages(),
        false,
        true,
        |space| {
            let entry = table
                .remove(handle)
                .expect("Tunnel Endpoint handle is pinned by the table lock");
            let installed = connection_state.leases[endpoint.side]
                .take()
                .expect("Tunnel close lost its mapping lease");
            assert_eq!(installed, lease, "Tunnel close lease changed before Commit");
            let published = space.commit_object_unmap(
                unmap
                    .take()
                    .expect("Tunnel close Unmap commits exactly once"),
            );
            let notice = commit_side_close(&endpoint, &mut connection_state);
            retire.install_notice(notice);
            (entry, published)
        },
    );
    let ((entry, published), synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            let translation =
                thread.process.space.lock().rollback_object_unmap(
                    unmap.take().expect("stale Tunnel close must roll back"),
                );
            drop(translation);
            return Err(SystemCallError::ObjectBusy);
        }
    };
    drop(connection_state);
    drop(table);
    drop(entry.into_parts()); // lifecycle 已由本事务提交，不重复调用对象 callback。
    completion.install(published);
    synchronization.start();
    Ok(plan)
}

pub(crate) fn close_detached(
    entry: handle::ProcessHandleEntry,
    owner: &Process,
) -> Result<(), handle::ProcessHandleEntry> {
    debug_assert!(owner.lifecycle.is_reapable());
    let endpoint = concrete_endpoint_arc(entry.object())
        .expect("Tunnel Endpoint entry must downcast to Endpoint");

    // 先结束 guard 临时量再进入分支；未完成分支会重取同一锁以回存状态，
    // 不得让 if-let scrutinee 把 guard 生命周期延长到分支体。
    let pending = { endpoint.detached_retire.lock().take() };
    if let Some(mut pending) = pending {
        if pending
            .change
            .advance(&owner.space, Some(pending.sink.as_ref()))
        {
            drop(entry.into_parts());
            return Ok(());
        }
        let previous = endpoint.detached_retire.lock().replace(pending);
        assert!(previous.is_none(), "detached Tunnel retire state raced");
        return Err(entry);
    }

    let lease = endpoint.connection.state.lock().leases[endpoint.side]
        .expect("detached Tunnel Endpoint must retain its mapping lease");
    let sink = match Arc::try_new(LeaseRetire::new(
        endpoint.connection.clone(),
        endpoint.clone(),
        lease,
    )) {
        Ok(sink) => sink,
        Err(_) => return Err(entry),
    };

    let mut connection_state = endpoint.connection.state.lock();
    assert_eq!(
        connection_state.leases[endpoint.side],
        Some(lease),
        "detached Tunnel close lease changed before Commit"
    );
    let (plan, pool) = {
        let mut space = owner.space.lock();
        let plan = match space.prepare_object_unmap(lease) {
            Ok(plan) => plan,
            Err(super::proc::SpaceError::Busy) => return Err(entry),
            Err(error) => panic!("detached Tunnel close invariant failed: {error:?}"),
        };
        let pool = Arc::clone(space.pool());
        (plan, pool)
    };
    let owners = match super::proc::supply_funded_table_frames(&pool, plan.table_budget()) {
        Ok(owners) => owners,
        Err(_) => {
            owner.space.lock().rollback_object_unmap_plan(plan);
            return Err(entry);
        }
    };
    let (prepared, mut space) = {
        let mut space = owner.space.lock();
        match space.complete_object_unmap(plan, owners) {
            Ok(prepared) => (prepared, space),
            Err((error, owners)) => {
                drop(space);
                drop(owners);
                panic!("detached Tunnel close funding invariant failed: {error:?}");
            }
        }
    };
    let installed = connection_state.leases[endpoint.side]
        .take()
        .expect("detached Tunnel close lost its mapping lease");
    assert_eq!(installed, lease, "detached Tunnel close lease changed");
    let published = space.commit_object_unmap(prepared);
    let notice = commit_side_close(&endpoint, &mut connection_state);
    sink.install_notice(notice);
    let change = space.begin_retire_published_change(published);
    drop(space);
    drop(connection_state);

    let mut pending = DetachedLeaseRetire { change, sink };
    if pending
        .change
        .advance(&owner.space, Some(pending.sink.as_ref()))
    {
        drop(entry.into_parts());
        Ok(())
    } else {
        let previous = endpoint.detached_retire.lock().replace(pending);
        assert!(
            previous.is_none(),
            "detached Tunnel retire state installed twice"
        );
        Err(entry)
    }
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

fn concrete_endpoint_arc(object: &ObjectRef) -> Result<Arc<Endpoint>, SystemCallError> {
    let any: Arc<dyn Any + Send + Sync> = object.clone();
    any.downcast::<Endpoint>()
        .map_err(|_| SystemCallError::WrongObjectType)
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
        super::proc::SpaceError::QuotaExceeded => SystemCallError::QuotaExceeded,
        super::proc::SpaceError::ReachLimit => SystemCallError::ReachLimit,
        super::proc::SpaceError::Conflict => SystemCallError::InvalidAddress,
        super::proc::SpaceError::Busy => SystemCallError::ObjectBusy,
        super::proc::SpaceError::Unbound => SystemCallError::ObjectNotAvailable,
    }
}
