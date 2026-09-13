//! Tunnel 对象：Connection 持帧，Endpoint 持本地映射 lease，Invitation
//! 是一次性可转移授权。不存在全局 id 或 registry。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
    tunnel::{
        TUNNEL_MAX_PAGES, TunnelAttachRequest, TunnelCreateRequest, TunnelCreateResult,
        TunnelEndpointResult,
    },
};
use memory_space::{BackingView, Protection, RegionKindView, RegionOwner, RetiringFragment};

use crate::{
    sync::Spinlock,
    task::{
        Thread, handle,
        object::{
            HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
            SubscribeResult,
        },
        proc::{
            MemoryRetireSink, ObjectMappingLease, PreparedMemoryChange, Process,
            map_shootdown_error, prepare_memory_completion,
        },
        wait::{Subscription, schedule_waiters},
    },
};

pub(crate) mod selftest;

enum SideState {
    Alive(Weak<Endpoint>),
    Invited(Weak<Invitation>),
    Closed,
}

struct ConnectionState {
    leases: [Option<ObjectMappingLease>; 2],
    sides: [SideState; 2],
}

struct Connection {
    /// 统一对象 core：identity、backing、可执行发布状态机与 metadata owner。
    /// 与公共 MemoryObject 共用同一类型，Tunnel 不再自己铸造对象身份。
    core: Arc<super::memory_object::MemoryObjectCore>,
    state: Spinlock<ConnectionState>,
    _metadata: super::resources::ConnectionPermit,
}

enum PeerNotice {
    Endpoint(Weak<Endpoint>),
    Invitation(Weak<Invitation>),
}

pub struct Endpoint {
    header: ObjectHeader,
    connection: Arc<Connection>,
    side: usize,
    closed: AtomicBool,
    wait: Spinlock<ObjectWaitState>,
    // 显式 close 在途时形成 Endpoint → LeaseRetire → Endpoint 的临时强环；
    // completion 完成分支先 take holder，再释放 handle entry，从而打破环。
    close_sink: Spinlock<Option<Arc<LeaseRetire>>>,
    _metadata: super::resources::EndpointPermit,
}

impl Endpoint {
    fn new(
        connection: Arc<Connection>,
        side: usize,
        metadata: super::resources::EndpointPermit,
    ) -> Result<Arc<Self>, SystemCallError> {
        let endpoint = Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            connection,
            side,
            closed: AtomicBool::new(false),
            wait: Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
            close_sink: Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        let close_sink = Arc::try_new(LeaseRetire::new(Arc::downgrade(&endpoint)))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        *endpoint.close_sink.lock() = Some(close_sink);
        Ok(endpoint)
    }

    fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    fn set_signals(&self, signals: ObjectSignals) {
        let clear = if signals.intersects(ObjectSignals::PEER_CLOSED) {
            ObjectSignals::PEER_ATTACHED
        } else {
            ObjectSignals::NONE
        };
        self.wait.lock().update(clear, signals);
        self.finish_waiters();
    }

    fn acknowledge_data(&self) {
        self.wait
            .lock()
            .update(ObjectSignals::DATA, ObjectSignals::NONE);
    }

    fn finish_waiters(&self) {
        schedule_waiters(&self.wait);
    }

    fn finish_close(&self, notice: Option<PeerNotice>) {
        self.wait.lock().update(
            ObjectSignals::DATA | ObjectSignals::PEER_ATTACHED,
            ObjectSignals::CLOSED,
        );
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
                SideState::Alive(endpoint) if core::ptr::eq(endpoint.as_ptr(), self)
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
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.wait.lock().complete_notification(reservation)
    }

    fn drain_waiters(&self, budget: usize) -> (usize, bool) {
        super::wait::drain_waiters(&self.wait, budget)
    }

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
            ObjectSignals::DATA
                | ObjectSignals::PEER_ATTACHED
                | ObjectSignals::PEER_CLOSED
                | ObjectSignals::CLOSED,
        )
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.wait.lock().rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.wait.lock().cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.wait.lock().unsubscribe(id);
        drop(retired);
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
    header: ObjectHeader,
    connection: Arc<Connection>,
    side: usize,
    closed: AtomicBool,
    _metadata: super::resources::InvitationPermit,
}

impl Invitation {
    fn new(
        connection: Arc<Connection>,
        side: usize,
        metadata: super::resources::InvitationPermit,
    ) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            connection,
            side,
            closed: AtomicBool::new(false),
            _metadata: metadata,
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
                SideState::Invited(invitation) if core::ptr::eq(invitation.as_ptr(), self)
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

fn reserve_mapping(
    connection: &Connection,
) -> Result<
    (
        memory_space::ObjectViewAuthorization,
        Vec<memory_space::WritePermit>,
    ),
    SystemCallError,
> {
    connection
        .core
        .authorize_write_view(Protection::ReadWrite)
        .map_err(super::memory_object::map_object_error)
}

fn cancel_writes(connection: &Connection, permits: Vec<memory_space::WritePermit>) {
    connection.core.cancel_writes(permits);
}

/// 建立本端 view 映射：锁外取得投影与 view 所有权，重入 AddressSpace 组装事务。
///
/// Create 与 Attach 共用它——两者只在对象来源与失败时的 Handle 回滚上不同，映射
/// 本身完全同形。整段独立成帧，避免两个调用点各自留下投影缓冲与预留元组。
#[inline(never)]
fn plan_side_mapping(
    connection: &Connection,
    thread: &Thread,
    placement: memory_space::MapPlacement,
    authorization: memory_space::ObjectViewAuthorization,
    permits: Vec<memory_space::WritePermit>,
) -> Result<
    (
        super::proc::MemoryChangePlan,
        Arc<super::memory_pool::MemoryPool>,
    ),
    super::proc::ObjectMapFailure,
> {
    let (intent, spans, view_owner) =
        match reserve_mapping_resources(connection, thread.process.resources.metadata(), placement)
        {
            Ok(reserved) => reserved,
            Err(error) => {
                return Err(super::proc::ObjectMapFailure {
                    error,
                    permits,
                    sources: None,
                });
            }
        };
    let mut space = thread.process.space.lock();
    let pool = Arc::clone(space.pool());
    space
        .plan_object_map(&intent, 0, &spans, authorization, permits, view_owner)
        .map(|plan| (plan, pool))
}

/// 两侧共用完整 Prepare，失败 owner 在 AddressSpace 锁外统一归还。
#[inline(never)]
fn prepare_side_mapping(
    connection: &Connection,
    thread: &Thread,
    placement: memory_space::MapPlacement,
) -> Result<PreparedMemoryChange, SystemCallError> {
    let (authorization, permits) = reserve_mapping(connection)?;
    let (plan, pool) =
        match plan_side_mapping(connection, thread, placement, authorization, permits) {
            Ok(plan) => plan,
            Err(failure) => {
                cancel_writes(connection, failure.permits);
                return Err(failure.error.into());
            }
        };
    let owners = match super::proc::fund_table_preflights(&pool, plan.preflights()) {
        Ok(owners) => owners,
        Err(error) => {
            let mut reclaimed = thread
                .process
                .space
                .lock()
                .rollback_memory_change_plan(plan);
            let permits = reclaimed.take_permits();
            drop(reclaimed);
            cancel_writes(connection, permits);
            return Err(error.into());
        }
    };
    let mut space = thread.process.space.lock();
    match space.complete_object_change(plan, owners) {
        Ok(prepared) => Ok(prepared),
        Err((failure, mut reclaimed)) => {
            drop(space);
            cancel_writes(connection, failure.permits);
            let permits = reclaimed.take_permits();
            drop(reclaimed);
            cancel_writes(connection, permits);
            Err(failure.error.into())
        }
    }
}

type ReservedMappingResources = (
    super::proc::MapIntent,
    Vec<(page_table::FrameNumber, usize)>,
    super::proc::PreparedObjectView,
);

/// 锁外预留 view 映射所需的全部 affine 资源：物理 span 投影（对象 backing 属
/// MEMORY_OBJECT 锁阶）与 view 所有权。进入 AddressSpace 后只做复检与组装。
fn reserve_mapping_resources(
    connection: &Connection,
    sponsor: &Arc<super::resources::MetadataSponsor>,
    placement: memory_space::MapPlacement,
) -> Result<ReservedMappingResources, super::proc::SpaceError> {
    let object_pages = connection.core.backing.pages();
    let mut spans = Vec::new();
    if spans
        .try_reserve_exact(connection.core.backing.projection_capacity())
        .is_err()
    {
        return Err(super::proc::SpaceError::NoFrame);
    }
    connection.core.backing.project(0, object_pages, &mut spans);
    let intent = super::proc::MapIntent::object_lease(
        placement,
        object_pages * super::proc::PAGE_SIZE,
        Protection::ReadWrite,
    );
    // view 所有权（对象强引用 + admission）在 Commit 前预留；使对象独立于 Handle
    // 与 Endpoint 存活，并与公共 MemoryObject 走同一条 view owner 路径。
    let view_owner = super::proc::PreparedObjectView::new(Arc::clone(&connection.core), sponsor)?;
    Ok((intent, spans, view_owner))
}

/// Commit 前放弃一份已 prepare 的 view 映射：摘出全部 owner，WritePermit 归还对象
/// 状态机，已准备的 translation 与表页 owner 在 AddressSpace 锁外析构。Create/Attach
/// 的每个提交前失败点都走这一条路径，不各自展开一份回滚序列。
#[inline(never)]
fn abandon_mapping(
    space: &super::proc::AddressSpace,
    connection: &Connection,
    prepared: PreparedMemoryChange,
) {
    let (permits, reclaimed) = {
        let mut guard = space.lock();
        let mut reclaimed = guard.rollback_memory_change(prepared);
        (reclaimed.take_permits(), reclaimed)
    };
    drop(reclaimed);
    cancel_writes(connection, permits);
}

/// Commit 前放弃一份已 prepare 的 view 撤销事务。撤销不预留 WritePermit（旧 permit
/// 随 retire 批次交回），因此只需归还 ledger reservation 与表页 owner。
#[inline(never)]
fn abandon_unmap(space: &super::proc::AddressSpace, prepared: PreparedMemoryChange) {
    let reclaimed = space.lock().rollback_memory_change(prepared);
    drop(reclaimed);
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
            SideState::Alive(candidate) if core::ptr::eq(candidate.as_ptr(), endpoint)
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

/// 退役 fragment 必须与 lease 记录的位置、对象内偏移与权限逐项一致；lease 本身
/// 只是复核凭据，真值在账本。
fn validate_retired_lease_fragment(lease: ObjectMappingLease, fragment: RetiringFragment) {
    assert!(
        fragment.range.start() >= lease.range.start()
            && fragment.range.end() <= lease.range.end()
            && fragment.owner == RegionOwner::Lease(lease.lease)
            && matches!(
                fragment.kind,
                RegionKindView::Mapping {
                    backing: BackingView::Object { object, offset },
                    current,
                    maximum,
                } if object == lease.object
                    && offset == lease.object_offset + fragment.range.start() - lease.range.start()
                    && current == lease.protection
                    && maximum == lease.protection
            ),
        "Tunnel retire fragment does not match its lease"
    );
}

struct LeaseRetireState {
    notice: Option<Option<PeerNotice>>,
    covered: [u64; (TUNNEL_MAX_PAGES as usize).div_ceil(64)],
    retired_pages: usize,
    expected_pages: usize,
}

struct LeaseRetire {
    endpoint: alloc::sync::Weak<Endpoint>,
    holder: Spinlock<Option<Arc<Endpoint>>>,
    lease: Spinlock<Option<ObjectMappingLease>>,
    state: Spinlock<LeaseRetireState>,
}

impl LeaseRetire {
    fn new(endpoint: alloc::sync::Weak<Endpoint>) -> Self {
        Self {
            endpoint,
            holder: Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            lease: Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            state: Spinlock::new(
                crate::sync::ranks::MEMORY_COMPLETION,
                LeaseRetireState {
                    notice: None,
                    covered: [0; (TUNNEL_MAX_PAGES as usize).div_ceil(64)],
                    retired_pages: 0,
                    expected_pages: 0,
                },
            ),
        }
    }

    fn configure_lease(&self, endpoint: Arc<Endpoint>, lease: ObjectMappingLease) {
        self.holder.lock().replace(endpoint);
        self.lease.lock().replace(lease);
        let mut state = self.state.lock();
        state.covered.fill(0);
        state.retired_pages = 0;
        state.expected_pages = lease.range.pages();
    }

    fn install_notice(&self, notice: Option<PeerNotice>) {
        let previous = self.state.lock().notice.replace(notice);
        assert!(previous.is_none(), "Tunnel close notice installed twice");
    }
}

impl MemoryRetireSink for LeaseRetire {
    fn retire_fragment(&self, fragment: RetiringFragment) {
        let lease = self
            .lease
            .lock()
            .as_ref()
            .copied()
            .expect("Tunnel retire fragment arrived before lease configuration");
        validate_retired_lease_fragment(lease, fragment);
        let mut state = self.state.lock();
        let first = (fragment.range.start() - lease.range.start()) / super::proc::PAGE_SIZE;
        for page in first..first + fragment.range.pages() {
            let bit = 1u64 << (page % 64);
            assert_eq!(
                state.covered[page / 64] & bit,
                0,
                "Tunnel lease page retired twice"
            );
            state.covered[page / 64] |= bit;
        }
        state.retired_pages += fragment.range.pages();
    }

    fn finish(&self) {
        let mut state = self.state.lock();
        assert!(
            state.retired_pages == state.expected_pages,
            "Tunnel lease completed before its complete range retired"
        );
        let notice = state
            .notice
            .take()
            .expect("Tunnel close retired before notice Commit");
        drop(state);
        let endpoint = self
            .holder
            .lock()
            .take()
            .or_else(|| self.endpoint.upgrade())
            .expect("Tunnel retire endpoint dropped before completion");
        endpoint.finish_close(notice);
    }
}

#[inline(never)]
fn new_connection(thread: &Thread, pages: usize) -> Result<Arc<Connection>, SystemCallError> {
    let pool = {
        let space = thread.process.space.lock();
        Arc::clone(space.pool())
    };
    let (core, connection_permit) = super::memory_object::MemoryObjectCore::new_tunnel_connection(
        &pool,
        thread.process.resources.metadata(),
        pages,
    )?;
    let core = Arc::try_new(core).map_err(|_| SystemCallError::OutOfMemory)?;
    Arc::try_new(Connection {
        core,
        state: Spinlock::new(
            crate::sync::ranks::CONNECTION,
            ConnectionState {
                leases: [None, None],
                sides: [SideState::Closed, SideState::Closed],
            },
        ),
        _metadata: connection_permit,
    })
    .map_err(|_| SystemCallError::OutOfMemory)
}

pub fn create(
    thread: &Thread,
    request: TunnelCreateRequest,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    if request.reserved != 0 || request.bytes == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let bytes = usize::try_from(request.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    let bytes = bytes
        .checked_add(super::proc::PAGE_SIZE - 1)
        .ok_or(SystemCallError::IllegalArgument)?;
    let pages = bytes / super::proc::PAGE_SIZE;
    if pages > TUNNEL_MAX_PAGES as usize {
        return Err(SystemCallError::ReachLimit);
    }
    let placement = super::proc::MapIntent::parse_placement(request.address, request.placement)?;
    let output =
        usize::try_from(request.result_address).map_err(|_| SystemCallError::IllegalArgument)?;
    let connection = new_connection(thread, pages)?;
    let sponsor = thread.process.resources.metadata();
    let endpoint = Endpoint::new(
        connection.clone(),
        0,
        super::resources::MetadataSponsor::reserve_endpoint(sponsor)?,
    )?;
    let invitation = Invitation::new(
        connection.clone(),
        1,
        super::resources::MetadataSponsor::reserve_invitation(sponsor)?,
    )?;

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

    let token = handle::transaction_token()?;
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
        if let Err(error) =
            space.check_range(output, core::mem::size_of::<TunnelCreateResult>(), true)
        {
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error.into());
        }
    }
    let mut connection_state = connection.state.lock();
    let mut mapping = match prepare_side_mapping(&connection, thread, placement) {
        Ok(prepared) => Some(prepared),
        Err(error) => {
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error);
        }
    };

    let (completion, plan) = match prepare_memory_completion(thread.process.clone(), 0, None, None)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            abandon_mapping(
                &thread.process.space,
                &connection,
                mapping.take().expect("TunnelCreate mapping exists"),
            );
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
            abandon_mapping(
                &thread.process.space,
                &connection,
                mapping.take().expect("TunnelCreate mapping exists"),
            );
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(map_shootdown_error(error));
        }
    };

    let lease_range = mapping
        .as_ref()
        .expect("TunnelCreate mapping exists")
        .published_lease()
        .range;
    let result = TunnelCreateResult {
        local: TunnelEndpointResult {
            endpoint: pair.owner,
            base: lease_range.start() as u64,
            bytes: lease_range.bytes() as u64,
        },
        invitation: pair.peer,
    };
    {
        let mut space = thread.process.space.lock();
        // SAFETY: TunnelCreateResult 无 padding；复检失败即杀本进程。Commit 尚未发生，
        // 因而失败路径仍可完整回滚 handle、permit、ledger 与 PTE reservation。
        if let Err(error) =
            unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &result) }
        {
            drop(space);
            abandon_mapping(
                &thread.process.space,
                &connection,
                mapping.take().expect("TunnelCreate mapping exists"),
            );
            table
                .rollback(reservation.take().expect("TunnelCreate reservation exists"))
                .expect("TunnelCreate reservation must remain owned");
            return Err(error);
        }
    }

    let committed = thread.process.space.commit_shootdown(
        &thread.process.lifecycle,
        shootdown,
        lease_range.start() / super::proc::PAGE_SIZE,
        lease_range.pages(),
        false,
        true,
        |space| {
            let (published, lease) = space.commit_view_map(
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
            abandon_mapping(
                &thread.process.space,
                &connection,
                mapping
                    .take()
                    .expect("stale TunnelCreate mapping must roll back"),
            );
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
    request: TunnelAttachRequest,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    if request.reserved != 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let placement = super::proc::MapIntent::parse_placement(request.address, request.placement)?;
    let output =
        usize::try_from(request.result_address).map_err(|_| SystemCallError::IllegalArgument)?;
    let invitation_handle = request.invitation;
    let token = handle::transaction_token()?;
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
    let endpoint = Endpoint::new(
        invitation.connection.clone(),
        invitation.side,
        super::resources::MetadataSponsor::reserve_endpoint(thread.process.resources.metadata())?,
    )?;
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
        if let Err(error) =
            space.check_range(output, core::mem::size_of::<TunnelEndpointResult>(), true)
        {
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
            SideState::Invited(candidate) if core::ptr::eq(candidate.as_ptr(), invitation)
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
    let mut mapping = match prepare_side_mapping(&invitation.connection, thread, placement) {
        Ok(prepared) => Some(prepared),
        Err(error) => {
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error);
        }
    };

    let (completion, plan) = match prepare_memory_completion(thread.process.clone(), 0, None, None)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            abandon_mapping(
                &thread.process.space,
                &invitation.connection,
                mapping.take().expect("TunnelAttach mapping exists"),
            );
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
            abandon_mapping(
                &thread.process.space,
                &invitation.connection,
                mapping.take().expect("TunnelAttach mapping exists"),
            );
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(map_shootdown_error(error));
        }
    };

    let lease_range = mapping
        .as_ref()
        .expect("TunnelAttach mapping exists")
        .published_lease()
        .range;
    let result = TunnelEndpointResult {
        endpoint: endpoint_handle,
        base: lease_range.start() as u64,
        bytes: lease_range.bytes() as u64,
    };
    {
        let mut space = thread.process.space.lock();
        // SAFETY: TunnelEndpointResult 无 padding；Commit 前复检失败按 fault 终止调用进程。
        if let Err(error) =
            unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &result) }
        {
            drop(space);
            abandon_mapping(
                &thread.process.space,
                &invitation.connection,
                mapping.take().expect("TunnelAttach mapping exists"),
            );
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(error);
        }
    }

    let committed = thread.process.space.commit_shootdown(
        &thread.process.lifecycle,
        shootdown,
        lease_range.start() / super::proc::PAGE_SIZE,
        lease_range.pages(),
        false,
        true,
        |space| {
            let consumed = table
                .remove(invitation_handle)
                .expect("TunnelAttach invitation is pinned by the table lock");
            let (published, lease) = space.commit_view_map(
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
            abandon_mapping(
                &thread.process.space,
                &invitation.connection,
                mapping
                    .take()
                    .expect("stale TunnelAttach mapping must roll back"),
            );
            table
                .rollback(reservation.take().expect("TunnelAttach reservation exists"))
                .expect("TunnelAttach reservation must remain owned");
            return Err(SystemCallError::ObjectBusy);
        }
    };
    endpoint.set_signals(ObjectSignals::PEER_ATTACHED);
    if let SideState::Alive(peer) = &connection_state.sides[1 - invitation.side]
        && let Some(peer) = peer.upgrade()
    {
        peer.set_signals(ObjectSignals::PEER_ATTACHED);
    }
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
            let plan = space
                .plan_object_unmap(lease)
                .map_err(SystemCallError::from)?;
            let pool = Arc::clone(space.pool());
            (plan, pool)
        };
        let owners = match super::proc::fund_table_preflights(&pool, plan.preflights()) {
            Ok(owners) => owners,
            Err(error) => {
                thread
                    .process
                    .space
                    .lock()
                    .rollback_memory_change_plan(plan);
                return Err(SystemCallError::from(error));
            }
        };
        let prepared = {
            let result = thread
                .process
                .space
                .lock()
                .complete_memory_change(plan, owners);
            match result {
                Ok(prepared) => prepared,
                Err((error, owners)) => {
                    drop(owners);
                    return Err(SystemCallError::from(error));
                }
            }
        };
        Some(prepared)
    };
    let retire = endpoint
        .close_sink
        .lock()
        .as_ref()
        .cloned()
        .expect("Tunnel endpoint close sink was not preallocated");
    let retire_sink: Arc<dyn MemoryRetireSink> = retire.clone();
    let (completion, plan) =
        match prepare_memory_completion(thread.process.clone(), 0, Some(retire_sink), None) {
            Ok(prepared) => prepared,
            Err(error) => {
                abandon_unmap(
                    &thread.process.space,
                    unmap.take().expect("Tunnel close Unmap exists"),
                );
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
            abandon_unmap(
                &thread.process.space,
                unmap.take().expect("Tunnel close Unmap exists"),
            );
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
            let published = space.commit_change(
                unmap
                    .take()
                    .expect("Tunnel close Unmap commits exactly once"),
            );
            retire.configure_lease(endpoint.clone(), lease);
            let notice = commit_side_close(&endpoint, &mut connection_state);
            retire.install_notice(notice);
            (entry, published)
        },
    );
    let ((entry, published), synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            abandon_unmap(
                &thread.process.space,
                unmap.take().expect("stale Tunnel close must roll back"),
            );
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

pub(crate) fn close_detached(entry: handle::ProcessHandleEntry, owner: &Process) {
    debug_assert!(owner.lifecycle.is_reapable());
    let endpoint = concrete_endpoint_arc(entry.object())
        .expect("Tunnel Endpoint entry must downcast to Endpoint");
    let notice = {
        let mut connection = endpoint.connection.state.lock();
        let lease = connection.leases[endpoint.side]
            .take()
            .expect("detached Tunnel close lost its mapping lease");
        assert_eq!(
            lease.object,
            endpoint.connection.core.identity(),
            "detached Tunnel close lease changed"
        );
        commit_side_close(&endpoint, &mut connection)
    };
    // REAPABLE 的执行门已排除用户页访问；handle 阶段之后的全地址空间 drain
    // 负责逐 region 归还 view owner 与 WritePermit，无需再建立单独 Unmap 事务。
    endpoint.finish_close(notice);
    drop(entry.into_parts());
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
