//! 扁平持久集合：成员节点只收提示，Receive 在来源锁外于集合锁之间重读电平。

use super::resources::{MetadataCharge, MetadataFunding, map_storage_error};
use super::{
    Thread, handle,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader},
    wait_source::{Hint, HintTarget, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    uaccess,
};
use alloc::sync::{Arc, Weak};
use core::{any::Any, pin::Pin};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectKind, ObjectSignals, Rights},
    wait::WaitResult,
    wait_set::{MemberRequest, RECEIVE_MAX, ReadyRecord},
};
use funded_storage::OwnedBox;
use maintenance::record::Outcome;
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

static NEXT_TOKEN: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

/// 唯一节点 owner；候选查询只复制 source/cookie，不能复制挂链资格。
struct Member {
    source: Arc<dyn ObservationSource>,
    node: WaitNode,
    request: MemberRequest,
    attached: bool,
}

impl Drop for Member {
    fn drop(&mut self) {
        if self.attached {
            // SAFETY: Member 自出生即 pin；集合移除/准入失败唯一持有本节点，
            // 来源 loan 到此返回才释放；调用方保证不持集合锁。
            drop(unsafe { self.source.detach(Pin::new_unchecked(&self.node)) });
        }
    }
}

struct Registration {
    member: Pin<OwnedBox<Member, MetadataCharge>>,
    queued: bool,
    previous: Option<u64>,
    next: Option<u64>,
}

struct State {
    members: OrderedTable<Registration, MetadataCharge>,
    limit: usize,
    head: Option<u64>,
    tail: Option<u64>,
    closed: bool,
    receiving: bool,
    /// Close 自摘与观察扫描交替，每步至多一个成员/扫描候选。
    cleanup_turn: bool,
}

impl State {
    fn enqueue(&mut self, token: u64) {
        let Some(entry) = self.members.get_mut(token) else {
            return;
        };
        if entry.queued {
            return;
        }
        entry.queued = true;
        entry.previous = self.tail;
        entry.next = None;
        if let Some(tail) = self.tail {
            self.members
                .get_mut(tail)
                .expect("candidate tail missing")
                .next = Some(token);
        } else {
            self.head = Some(token);
        }
        self.tail = Some(token);
    }

    fn unlink(&mut self, token: u64) {
        let Some(entry) = self.members.get_mut(token) else {
            return;
        };
        if !entry.queued {
            return;
        }
        let previous = entry.previous.take();
        let next = entry.next.take();
        entry.queued = false;
        if let Some(previous) = previous {
            self.members
                .get_mut(previous)
                .expect("candidate predecessor missing")
                .next = next;
        } else {
            self.head = next;
        }
        if let Some(next) = next {
            self.members
                .get_mut(next)
                .expect("candidate successor missing")
                .previous = previous;
        } else {
            self.tail = previous;
        }
    }

    fn remove(&mut self, token: u64) -> Option<PreparedEntry<Registration, MetadataCharge>> {
        self.unlink(token);
        self.members.remove(token)
    }
}

pub struct WaitSet {
    holders: capability::Holders,
    header: ObjectHeader,
    state: Spinlock<State>,
    source: WaitQueue,
    this: Spinlock<Weak<Self>>,
    _metadata: super::resources::MetadataCharge,
}

impl WaitSet {
    fn new(limit: usize, pool: Arc<MemoryPool>) -> Result<Arc<Self>, SystemCallError> {
        // 外部 ABI 容量为 u32；AVL 查找/单步删除的树高由键宽及容量界定。
        if limit == 0 || limit > u32::MAX as usize {
            return Err(SystemCallError::IllegalArgument);
        }
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let set = Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            state: Spinlock::new(
                ranks::WAIT_SET,
                State {
                    members: OrderedTable::new(limit),
                    limit,
                    head: None,
                    tail: None,
                    closed: false,
                    receiving: false,
                    cleanup_turn: true,
                },
            ),
            source: WaitQueue::new(pool.maintenance_account().clone()),
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *set.this.lock() = Arc::downgrade(&set);
        Ok(set)
    }

    fn root(&self) -> Arc<Self> {
        self.this
            .lock()
            .upgrade()
            .expect("live WaitSet lost storage root")
    }

    /// 调用者持 state 锁，READABLE 与候选链/关闭同域提交。
    fn update(&self, state: &State) -> bool {
        let level = if state.closed {
            ObjectSignals::CLOSED
        } else if state.head.is_some() {
            ObjectSignals::READABLE
        } else {
            ObjectSignals::NONE
        };
        // SAFETY: WaitSet 位于稳定 Arc，全部 source 操作共用 state 锁。
        unsafe { self.source.update(level, state.closed) }
    }

    fn kick(self: &Arc<Self>) {
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }

    fn add(
        self: &Arc<Self>,
        source: Arc<dyn ObservationSource>,
        request: MemberRequest,
    ) -> Result<u64, SystemCallError> {
        let token = NEXT_TOKEN.allocate().ok_or(SystemCallError::ReachLimit)?;
        let member = OwnedBox::try_new(
            Member {
                source,
                node: WaitNode::new(),
                request,
                attached: false,
            },
            &MetadataFunding(self._metadata.payer()),
        )
        .map_err(|(error, _)| map_storage_error(error))?
        .into_pin();
        let mut prepared = PreparedEntry::try_new(
            token,
            Registration {
                member,
                queued: false,
                previous: None,
                next: None,
            },
            &MetadataFunding(self._metadata.payer()),
        )
        .map_err(|error| match error {
            InsertError::Limit(_) => SystemCallError::ReachLimit,
            InsertError::Storage(error, _) => map_storage_error(error),
        })?;
        let target: Arc<dyn HintTarget> = self.clone();
        // SAFETY: 此时本栈独占待提交成员，node pin 地址已固定；失败时同一 owner 自摘。
        unsafe {
            let member = prepared.value_mut().member.as_mut().get_unchecked_mut();
            member.source.attach(
                Pin::new_unchecked(&member.node),
                request.signals,
                Hint::new(Arc::downgrade(&target), token),
            );
            member.attached = true;
        }
        let committed = {
            let mut state = self.state.lock();
            if state.closed {
                Err(SystemCallError::ObjectClosed)
            } else if state.members.len() == state.limit {
                Err(SystemCallError::ReachLimit)
            } else {
                state.members.insert_prepared(prepared);
                // Add 无条件成为一次候选：Receive 重读实际电平，覆盖 attach 前后的所有窗口。
                state.enqueue(token);
                self.update(&state);
                drop(state);
                self.kick();
                return Ok(token);
            }
        };
        // prepared 的 Drop 必须在集合锁外，来源自摘不能嵌套目标锁。
        drop(prepared);
        committed
    }

    fn remove(self: &Arc<Self>, token: u64) -> Result<(), SystemCallError> {
        let (removed, changed) = {
            let mut state = self.state.lock();
            let removed = state.remove(token).ok_or(SystemCallError::ObjectNotFound)?;
            (removed, self.update(&state))
        };
        drop(removed);
        if changed {
            self.kick();
        }
        Ok(())
    }

    fn close(&self) {
        let root = self.root();
        {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            self.update(&state);
        }
        root.kick();
    }

    fn receive(
        self: &Arc<Self>,
        capacity: usize,
        pool: &Arc<MemoryPool>,
    ) -> Result<super::resources::MetadataBuffer<ReadyRecord>, SystemCallError> {
        let funding = MetadataFunding(pool);
        let mut candidates = super::resources::MetadataBuffer::try_empty(capacity, &funding)
            .map_err(super::resources::map_storage_error)?;
        let mut records = super::resources::MetadataBuffer::try_empty(capacity, &funding)
            .map_err(super::resources::map_storage_error)?;
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(SystemCallError::ObjectClosed);
            }
            if state.receiving {
                return Err(SystemCallError::ObjectBusy);
            }
            state.receiving = true;
            // 先摘本批所有候选，永久就绪成员本次不会被重复领取。
            for _ in 0..capacity {
                let Some(token) = state.head else { break };
                state.unlink(token);
                let member = &state
                    .members
                    .get(token)
                    .expect("candidate member missing")
                    .member;
                candidates.push((token, member.source.clone(), member.request));
            }
            self.update(&state);
        }
        let _receive = ReceiveGuard(self);
        for (token, source, request) in candidates {
            let snapshot = source.snapshot();
            let observed = if snapshot.closed {
                (snapshot.signals & request.signals) | ObjectSignals::CLOSED
            } else {
                snapshot.signals & request.signals
            };
            let mut state = self.state.lock();
            if !state.closed && state.members.contains_key(token) && observed != ObjectSignals::NONE
            {
                state.enqueue(token);
                let result = WaitResult::from_observed(observed);
                records.push(ReadyRecord {
                    token,
                    cookie: request.cookie,
                    observed,
                    reason: result.reason,
                    reserved: 0,
                });
            }
            // query 期间的新 hint 已在同锁置候选；电平不满足也不清掉这个新提示。
            self.update(&state);
        }
        // 无论电平是否重置，都归还本次活动，闭合与来源扫描交错的 notified 握手。
        self.kick();
        Ok(records)
    }
}

struct ReceiveGuard<'a>(&'a Arc<WaitSet>);
impl Drop for ReceiveGuard<'_> {
    fn drop(&mut self) {
        self.0.state.lock().receiving = false;
    }
}

impl HintTarget for WaitSet {
    fn hint(&self, token: u64) {
        let root = self.root();
        let changed = {
            let mut state = self.state.lock();
            if state.closed || !state.members.contains_key(token) {
                return;
            }
            state.enqueue(token);
            self.update(&state)
        };
        if changed {
            root.kick();
        }
    }
}

impl ObservationSource for WaitSet {
    fn snapshot(&self) -> Snapshot {
        let _state = self.state.lock();
        // SAFETY: 稳定 core，持同域锁。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.state.lock();
        // SAFETY: 调用方保持节点/来源loan，锁串行所有链操作。
        unsafe { self.source.attach(node, interest, hint) }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.state.lock();
        // SAFETY: 同挂链契约；返回凭据无节点借用。
        unsafe { self.source.detach(node) }
    }
}

impl Work for WaitSet {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(&self) -> Outcome {
        let (removed, scan) = {
            let mut state = self.state.lock();
            if state.closed && state.cleanup_turn && !state.members.is_empty() {
                let token = *state
                    .members
                    .next_after::<u64>(None)
                    .expect("nonempty member table lost minimum")
                    .0;
                let removed = state.remove(token);
                state.cleanup_turn = false;
                (removed, None)
            } else {
                state.cleanup_turn = true;
                // SAFETY: state 锁保护 source；来源只复制 Delivery。
                (None, Some(unsafe { self.source.step() }))
            }
        };
        if removed.is_some() {
            drop(removed);
            return Outcome::Ready;
        }
        match scan.expect("WaitSet work had no action") {
            Step::Hint(delivery) => {
                delivery.send();
                Outcome::Ready
            }
            Step::Progress => Outcome::Ready,
            Step::Idle => {
                let state = self.state.lock();
                if state.closed && !state.members.is_empty() {
                    Outcome::Ready
                } else {
                    Outcome::Idle
                }
            }
        }
    }
}

impl KernelObject for WaitSet {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::WaitSet
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::READABLE | ObjectSignals::CLOSED)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.root())
    }
    fn on_last_capability(&self) {
        self.close();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn resolve(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<Arc<WaitSet>, SystemCallError> {
    let object = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let table = body.handles.lock();
        handle::resolve(&table, handle, rights, ObjectKind::WaitSet)?
    };
    let any: Arc<dyn Any + Send + Sync> = object;
    any.downcast::<WaitSet>()
        .map_err(|_| SystemCallError::WrongObjectType)
}

pub fn create(
    thread: &Thread,
    limit: usize,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let set = WaitSet::new(limit, thread.process.funding_pool())?;
    let caps = [handle::mint(set, rights)?];
    handle::install(
        thread,
        caps,
        |handles| uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}

pub fn register(
    thread: &Thread,
    set_handle: Handle,
    input: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let (request, object) = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let request: MemberRequest = {
            let mut vm = body.vm.space.lock();
            let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
            // SAFETY: 固定宽整数 ABI 的任意位型有效，随后校验 mask/kind/rights。
            unsafe { uaccess::read_user_value(space, input) }?
        };
        let table = body.handles.lock();
        let object = table
            .get(request.handle, Rights::WAIT)
            .map_err(handle::map_error)?
            .object()
            .0
            .clone();
        (request, object)
    };
    if object.kind() == ObjectKind::WaitSet {
        return Err(SystemCallError::WrongObjectType);
    }
    let allowed = object
        .allowed_signals()
        .ok_or(SystemCallError::WrongObjectType)?;
    if request.signals == ObjectSignals::NONE || request.signals.raw() & !allowed.raw() != 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let source = object
        .wait_source()
        .ok_or(SystemCallError::WrongObjectType)?;
    let set = resolve(thread, set_handle, Rights::MANAGE)?;
    let token = set.add(source, request)?;
    if let Err(error) = uaccess::write_user(thread, output, &token) {
        // 与 Remove/Close 竞争时，它们已接管相同成员的 detach；不重复释放。
        let _ = set.remove(token);
        return Err(error);
    }
    Ok(())
}

pub fn remove(thread: &Thread, handle: Handle, token: u64) -> Result<(), SystemCallError> {
    resolve(thread, handle, Rights::MANAGE)?.remove(token)
}

pub fn receive(
    thread: &Thread,
    handle: Handle,
    output: usize,
    capacity: usize,
    count_output: usize,
) -> Result<(), SystemCallError> {
    if capacity == 0 || capacity > RECEIVE_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let records =
        resolve(thread, handle, Rights::READ)?.receive(capacity, &thread.process.funding_pool())?;
    // 纯电平记录不被消费；uaccess 失败时满足项仍在候选队尾，下次可重新读取。
    for (index, record) in records.iter().enumerate() {
        let address = output
            .checked_add(index * core::mem::size_of::<ReadyRecord>())
            .ok_or(SystemCallError::IllegalArgument)?;
        uaccess::write_user(thread, address, record)?;
    }
    uaccess::write_user(thread, count_output, &(records.len() as u32))
}
