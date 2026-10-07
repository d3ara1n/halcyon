//! Tunnel：Connection 拥有 backing root，Endpoint/Invitation 只拥有事件与接入授权。
//! 普通 View 独立持 use；最后 authority 关闭不隐式撤映射。

use super::{
    Thread,
    handle::{self, Invocation},
    memory_object::MemoryObjectCore,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectKind},
    wait_source::{Hint, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    vm::backing::BackingOwner,
};
use alloc::sync::{Arc, Weak};
use core::{any::Any, pin::Pin};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    tunnel::{TUNNEL_MAX_PAGES, TunnelCreateRequest, TunnelCreateResult},
};

const ENDPOINT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::SIGNAL.raw()
        | Rights::MANAGE.raw(),
);
const INVITATION_RIGHTS: Rights =
    Rights::from_raw(ENDPOINT_RIGHTS.raw() | Rights::TRANSIT.raw() | Rights::GRANT.raw());
const EVENTS: ObjectSignals = ObjectSignals::from_raw(
    ObjectSignals::DATA.raw()
        | ObjectSignals::PEER_ATTACHED.raw()
        | ObjectSignals::PEER_CLOSED.raw()
        | ObjectSignals::CLOSED.raw(),
);

enum Side {
    Prepared,
    Invited(Weak<Invitation>),
    Attached(Weak<Endpoint>),
    Closed,
}
struct State {
    sides: [Side; 2],
    owner: Option<BackingOwner>,
}
struct Connection {
    core: Arc<MemoryObjectCore>,
    state: Spinlock<State>,
    _metadata: super::resources::MetadataCharge,
}
impl Connection {
    fn retire_if_closed(state: &mut State) -> Option<BackingOwner> {
        if state.sides.iter().all(|side| matches!(side, Side::Closed)) {
            state.owner.take()
        } else {
            None
        }
    }
    /// 持 connection 锁，只复制 peer 强根；事件提交与业务状态共用该锁。
    fn peer(state: &State, side: usize) -> Option<Arc<Endpoint>> {
        match &state.sides[1 - side] {
            Side::Attached(peer) => peer.upgrade(),
            _ => None,
        }
    }
}

pub struct Endpoint {
    header: ObjectHeader,
    holders: capability::Holders,
    connection: Arc<Connection>,
    side: usize,
    source: WaitQueue,
    this: Spinlock<Weak<Self>>,
    _metadata: super::resources::MetadataCharge,
}
impl Endpoint {
    fn new(
        connection: Arc<Connection>,
        side: usize,
        pool: &Arc<MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(pool)?;
        let endpoint = Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            source: WaitQueue::new(pool.maintenance_account().clone()),
            connection,
            side,
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *endpoint.this.lock() = Arc::downgrade(&endpoint);
        Ok(endpoint)
    }
    fn kick(&self) {
        let root: Arc<dyn Work> = self
            .this
            .lock()
            .upgrade()
            .expect("live endpoint lost source owner");
        mt::publish(root);
        mt::kick();
    }
    fn live(&self, state: &State) -> bool {
        matches!(&state.sides[self.side], Side::Attached(candidate) if core::ptr::eq(candidate.as_ptr(), self))
    }
    /// 调用者必须持 Connection 锁；只变更电平，来源扫描锁外激活。
    unsafe fn update(&self, set: ObjectSignals, clear: ObjectSignals, closed: bool) -> bool {
        // SAFETY: 上述唯一同步域覆盖来源快照、节点与电平；关闭不能被 DATA 覆盖。
        unsafe {
            let snapshot = self.source.snapshot();
            if snapshot.closed {
                return false;
            }
            self.source
                .update((snapshot.signals & !clear) | set, closed)
        }
    }
    pub(crate) fn core(&self) -> Arc<MemoryObjectCore> {
        self.connection.core.clone()
    }
    fn close(&self) {
        let (own_hint, peer, peer_hint, retired) = {
            let mut state = self.connection.state.lock();
            if !self.live(&state) {
                return;
            } // 未公开对象的准备失败不关闭真实侧。
            state.sides[self.side] = Side::Closed;
            let peer = Connection::peer(&state, self.side);
            // SAFETY: connection 锁同时拥有两侧业务与来源状态。
            let own_hint = unsafe {
                self.update(
                    ObjectSignals::CLOSED,
                    ObjectSignals::DATA | ObjectSignals::PEER_ATTACHED,
                    true,
                )
            };
            let peer_hint = peer.as_ref().is_some_and(|peer| unsafe {
                peer.update(
                    ObjectSignals::PEER_CLOSED,
                    ObjectSignals::PEER_ATTACHED,
                    false,
                )
            });
            (
                own_hint,
                peer,
                peer_hint,
                Connection::retire_if_closed(&mut state),
            )
        };
        drop(retired);
        if own_hint {
            self.kick();
        }
        if peer_hint {
            peer.expect("hint lost peer owner").kick();
        }
    }
    fn notify(&self) -> Result<(), SystemCallError> {
        let (peer, hint) = {
            let state = self.connection.state.lock();
            if !self.live(&state) {
                return Err(SystemCallError::ObjectClosed);
            }
            let peer = match &state.sides[1 - self.side] {
                Side::Attached(peer) => peer.upgrade().ok_or(SystemCallError::ObjectClosed)?,
                Side::Invited(_) => return Err(SystemCallError::ObjectNotAvailable),
                _ => return Err(SystemCallError::ObjectClosed),
            };
            // SAFETY: peer 与本端同属 connection 同步域；此刻双方均 Attached。
            let hint = unsafe { peer.update(ObjectSignals::DATA, ObjectSignals::NONE, false) };
            (peer, hint)
        };
        if hint {
            peer.kick();
        }
        Ok(())
    }
    fn acknowledge(&self) -> Result<(), SystemCallError> {
        let hint = {
            let state = self.connection.state.lock();
            if !self.live(&state) {
                return Err(SystemCallError::ObjectClosed);
            }
            // SAFETY: 持 connection 域，仅清本端 DATA。
            unsafe { self.update(ObjectSignals::NONE, ObjectSignals::DATA, false) }
        };
        if hint {
            self.kick();
        }
        Ok(())
    }
}
impl KernelObject for Endpoint {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::TunnelEndpoint
    }
    fn related_id(&self) -> u64 {
        self.connection.core.identity().get()
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(ENDPOINT_RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(EVENTS)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        self.this
            .lock()
            .upgrade()
            .map(|source| source as Arc<dyn ObservationSource>)
    }
    fn on_last_capability(&self) {
        self.close();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
impl ObservationSource for Endpoint {
    fn snapshot(&self) -> Snapshot {
        let _state = self.connection.state.lock();
        // SAFETY: 来源的唯一同步域。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.connection.state.lock();
        // SAFETY: 调用者保证 pin/loan；connection 域保护挂链。
        unsafe {
            self.source.attach(node, interest, hint);
        }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.connection.state.lock();
        // SAFETY: 同挂链契约，返回凭据锁外析构。
        unsafe { self.source.detach(node) }
    }
}
impl Work for Endpoint {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let step = {
            let _state = self.connection.state.lock();
            // SAFETY: 单候选扫描在来源域内完成，回调锁外运行。
            unsafe { self.source.step() }
        };
        use maintenance::record::Outcome;
        match step {
            Step::Idle => Outcome::Idle,
            Step::Progress => Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                Outcome::Ready
            }
        }
    }
}

pub struct Invitation {
    header: ObjectHeader,
    holders: capability::Holders,
    connection: Arc<Connection>,
    side: usize,
    _metadata: super::resources::MetadataCharge,
}
impl Invitation {
    fn new(connection: Arc<Connection>, side: usize) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(connection._metadata.payer())?;
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            connection,
            side,
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    fn pending(&self, state: &State) -> bool {
        matches!(&state.sides[self.side], Side::Invited(candidate) if core::ptr::eq(candidate.as_ptr(), self))
    }
    fn abandon(&self) {
        let (peer, hint, retired) = {
            let mut state = self.connection.state.lock();
            if !self.pending(&state) {
                return;
            } // Attach 已移交或从未公开。
            state.sides[self.side] = Side::Closed;
            let peer = Connection::peer(&state, self.side);
            // SAFETY: 对端电平与侧状态同域提交。
            let hint = peer.as_ref().is_some_and(|peer| unsafe {
                peer.update(
                    ObjectSignals::PEER_CLOSED,
                    ObjectSignals::PEER_ATTACHED,
                    false,
                )
            });
            (peer, hint, Connection::retire_if_closed(&mut state))
        };
        drop(retired);
        if hint {
            peer.expect("invitation lost notified peer").kick();
        }
    }
}
impl KernelObject for Invitation {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::TunnelInvitation
    }
    fn related_id(&self) -> u64 {
        self.connection.core.identity().get()
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(INVITATION_RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        None
    }
    fn on_last_capability(&self) {
        self.abandon();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(thread: &Thread, input: usize) -> Result<(), SystemCallError> {
    let request: TunnelCreateRequest = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        // SAFETY: 请求为固定宽整数，后续检查所有保留位、长度和权限。
        unsafe {
            crate::uaccess::read_user_value(
                vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
                input,
            )
        }?
    };
    let bytes = usize::try_from(request.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    let range =
        memory_space::PageRange::rounded(0, bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    if request.reserved != 0 || range.pages() as u64 > TUNNEL_MAX_PAGES {
        return Err(SystemCallError::IllegalArgument);
    }
    let output =
        usize::try_from(request.result_address).map_err(|_| SystemCallError::IllegalArgument)?;
    let funding = super::handle::Authorization::accept(thread, request.pool, Rights::CREATE)?;
    let pool = MemoryPool::concrete(funding.object())?;
    let (core, owner) = MemoryObjectCore::new(pool.clone(), range.pages())?;
    let metadata = super::resources::MetadataCharge::arc::<Connection>(&pool)?;
    let connection = Arc::try_new(Connection {
        core,
        _metadata: metadata,
        state: Spinlock::new(
            ranks::CONNECTION,
            State {
                sides: [Side::Prepared, Side::Prepared],
                owner: Some(owner),
            },
        ),
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let endpoint = Endpoint::new(connection.clone(), 0, &pool)?;
    let invitation = Invitation::new(connection.clone(), 1)?;
    let endpoint_cap = handle::mint(endpoint.clone(), ENDPOINT_RIGHTS)?;
    let invitation_cap = match handle::mint(invitation.clone(), INVITATION_RIGHTS) {
        Ok(cap) => cap,
        Err(error) => {
            handle::release_all([endpoint_cap]);
            return Err(error);
        }
    };
    let caps = [endpoint_cap, invitation_cap];
    handle::install(
        thread,
        caps,
        |handles| {
            crate::uaccess::write_user(
                thread,
                output,
                &TunnelCreateResult {
                    endpoint: handles[0],
                    invitation: handles[1],
                    bytes: range.bytes() as u64,
                    reserved: 0,
                },
            )
        },
        || {
            connection.state.lock().sides = [
                Side::Attached(Arc::downgrade(&endpoint)),
                Side::Invited(Arc::downgrade(&invitation)),
            ];
        },
    )
    .map(|_| ())
}

/// 邀请的唯一在途 owner 在 commit 前保留原槽；成功后只交付一个 Endpoint。
pub fn attach(thread: &Thread, handle: Handle) -> Result<Handle, SystemCallError> {
    let mut authority = Invocation::claim(thread, handle, Rights::MANAGE)?;
    let object = authority.object().clone();
    let invitation = object
        .as_any()
        .downcast_ref::<Invitation>()
        .ok_or(SystemCallError::WrongObjectType)?;
    let endpoint = Endpoint::new(
        invitation.connection.clone(),
        invitation.side,
        &thread.process.funding_pool(),
    )?;
    let rights = authority.cap().rights() & ENDPOINT_RIGHTS;
    let caps = [handle::mint(endpoint.clone(), rights)?];
    let reserved = authority
        .body()
        .handles
        .lock()
        .reserve(1, &super::resources::MetadataFunding(thread.process.pool()));
    let reserved = match reserved {
        Ok(value) => value,
        Err(error) => {
            handle::release_all(caps);
            return Err(handle::map_error(error));
        }
    };
    let handle = reserved.handles()[0];
    let committed = {
        let mut state = invitation.connection.state.lock();
        if !invitation.pending(&state)
            || !matches!(&state.sides[1 - invitation.side], Side::Attached(_))
        {
            None
        } else {
            state.sides[invitation.side] = Side::Attached(Arc::downgrade(&endpoint));
            let peer = Connection::peer(&state, invitation.side);
            // SAFETY: 双方来源均属本 connection；Attach 不证明双方已有映射。
            let own_hint = unsafe {
                endpoint.update(ObjectSignals::PEER_ATTACHED, ObjectSignals::NONE, false)
            };
            let peer_hint = peer.as_ref().is_some_and(|peer| unsafe {
                peer.update(ObjectSignals::PEER_ATTACHED, ObjectSignals::NONE, false)
            });
            Some((peer, own_hint, peer_hint))
        }
    };
    let Some((peer, own_hint, peer_hint)) = committed else {
        authority.body().handles.lock().rollback(reserved);
        handle::release_all(caps);
        return Err(SystemCallError::ObjectClosed);
    };
    // 业务已提交，所有后续步骤必成；消费 Invitation 的归零不再 abandon 该侧。
    authority.consume();
    let installed = authority.body().handles.lock().commit(reserved, caps);
    match installed {
        Ok(capability::Commit::Installed(_)) => {}
        Ok(capability::Commit::Released { events, .. }) => {
            for event in events {
                super::cap::deliver(Some(event));
            }
        }
        Err((caps, _)) => {
            handle::release_all(caps);
            unreachable!("owned TunnelAttach reservation changed");
        }
    }
    if own_hint {
        endpoint.kick();
    }
    if peer_hint {
        peer.expect("attached peer vanished").kick();
    }
    Ok(handle)
}

fn with_endpoint(
    thread: &Thread,
    handle: Handle,
    action: impl FnOnce(&Endpoint) -> Result<(), SystemCallError>,
) -> Result<(), SystemCallError> {
    let authority = super::handle::Authorization::accept(thread, handle, Rights::SIGNAL)?;
    let endpoint = authority
        .object()
        .as_any()
        .downcast_ref::<Endpoint>()
        .ok_or(SystemCallError::WrongObjectType)?;
    action(endpoint)
}
pub fn notify(thread: &Thread, handle: Handle) -> Result<(), SystemCallError> {
    with_endpoint(thread, handle, Endpoint::notify)
}
pub fn acknowledge_data(thread: &Thread, handle: Handle) -> Result<(), SystemCallError> {
    with_endpoint(thread, handle, Endpoint::acknowledge)
}
