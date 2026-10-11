//! 文件订阅的弱目标身份、独立付款与序号；控制寿命由原请求任务持有。
use crate::{
    authority::{AccessSnapshot, GrantLease},
    protocol::{
        Status, WATCH_BUSINESS, WATCH_INITIAL, WATCH_TERMINAL, WATCH_UNBOUND, WatchInfo,
        WatchReason, WatchState,
    },
    resource::FsResource,
    store::{NodeId, NodeState},
};
use alloc::{rc::Rc, sync::Arc};
use core::mem::ManuallyDrop;
use erhino_shared::call::SystemCallError;
use libbudget::Charge;
use libkernel::ipc::notification::{NotificationOwner, NotificationSignaler};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

/// mask 只筛选业务提示；Initial/Terminal 始终属于订阅。
#[derive(Clone, Copy)]
pub struct Sequence {
    mask: u64,
    info: WatchInfo,
}
impl Sequence {
    pub fn new(mask: u64) -> Result<Self, Status> {
        if mask & !WATCH_BUSINESS != 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            mask,
            info: WatchInfo {
                sequence: 1,
                pending: WATCH_INITIAL,
                state: WatchState::Active,
                reason: WatchReason::Active,
            },
        })
    }
    pub fn info(&self) -> WatchInfo {
        self.info
    }
    pub fn change(&mut self, bits: u64) {
        let bits = bits & self.mask;
        if bits == 0 || self.info.state == WatchState::Terminal {
            return;
        }
        if self.info.sequence == u64::MAX - 1 {
            self.terminate(WatchReason::SequenceExhausted);
            return;
        }
        self.info.sequence += 1;
        self.info.pending |= bits;
    }
    pub fn terminate(&mut self, reason: WatchReason) {
        if self.info.state == WatchState::Terminal {
            return;
        }
        self.info.sequence = self
            .info
            .sequence
            .checked_add(1)
            .expect("active file watch exhausted terminal sequence");
        self.info.pending |= WATCH_TERMINAL;
        self.info.state = WatchState::Terminal;
        self.info.reason = reason;
    }
    pub fn ack(&mut self, sequence: u64) -> Result<WatchInfo, Status> {
        if sequence > self.info.sequence {
            return Err(Status::Invalid);
        }
        if sequence == self.info.sequence {
            self.info.pending = 0;
        }
        Ok(self.info)
    }
}

/// 与内核通知角色分开的逻辑状态；所有终态在此释放选择与授权 token。
struct State {
    target: Option<NodeId>,
    authority: Option<Rc<GrantLease>>,
    sequence: Sequence,
    unbound: bool,
}
impl State {
    fn terminate(&mut self, reason: WatchReason) {
        self.sequence.terminate(reason);
        self.target = None;
        self.authority = None;
    }
    fn change(&mut self, bits: u64) {
        if bits & WATCH_UNBOUND != 0 {
            self.unbound = true;
        }
        self.sequence.change(bits);
        let info = self.sequence.info();
        if info.state == WatchState::Terminal {
            self.terminate(info.reason);
        }
    }
}
struct Watch {
    state: State,
    funding: crate::Funding,
    signaled: u64,
    notification_closed: bool,
    signaler: NotificationSignaler,
    owner: NotificationOwner,
    _slot: Permit,
    _unit: Charge,
    _bytes: Charge,
    _source: Charge,
}
impl Watch {
    fn terminate(&mut self, reason: WatchReason) {
        self.state.terminate(reason);
    }
    fn change(&mut self, bits: u64) {
        self.state.change(bits);
    }
}
pub struct PreparedWatch {
    entry: PreparedEntry<Watch, ()>,
}
impl PreparedWatch {
    pub fn context(&self) -> u64 {
        self.entry.key()
    }
    pub fn info(&self) -> WatchInfo {
        self.entry.value().state.sequence.info()
    }
    pub fn is_authorized(&self) -> bool {
        self.entry
            .value()
            .state
            .authority
            .as_ref()
            .is_some_and(|lease| lease.active.get())
    }
}

/// NodeId 单调不复用，只引用已鉴权选定目标；表不增加 NodeRef 的业务 pin。
pub struct WatchTable {
    records: ManuallyDrop<OrderedTable<Watch, ()>>,
    slots: Arc<Counter>,
    sealed: bool,
}
fn system(error: SystemCallError) -> Status {
    match error {
        SystemCallError::QuotaExceeded => Status::Quota,
        _ => Status::Resource,
    }
}
impl WatchTable {
    pub fn new(limit: usize) -> Result<Self, Status> {
        if limit == 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            records: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            sealed: false,
        })
    }
    pub fn prepare(
        &self,
        context: u64,
        access: &AccessSnapshot,
        target: NodeId,
        mask: u64,
        owner: NotificationOwner,
        signaler: NotificationSignaler,
    ) -> Result<PreparedWatch, Status> {
        if self.sealed || !access.is_active() {
            return Err(Status::Revoked);
        }
        if context == 0 || self.records.get(context).is_some() {
            return Err(Status::Invalid);
        }
        let account = access.account();
        let slot = Counter::try_acquire(&self.slots).map_err(|_| Status::Quota)?;
        let unit = account.acquire(FsResource::Watch, 1).map_err(system)?;
        let bytes = account
            .acquire(
                FsResource::Bytes,
                PreparedEntry::<Watch, ()>::allocation_bytes(),
            )
            .map_err(system)?;
        // A 方案只有控制 Lifetime 来源；Observer 消散不结束订阅。
        let source = account.acquire(FsResource::WaitSource, 1).map_err(system)?;
        let watch = Watch {
            state: State {
                target: Some(target),
                authority: Some(access.lease.clone()),
                sequence: Sequence::new(mask)?,
                unbound: false,
            },
            funding: access.funding().clone(),
            signaled: 0,
            notification_closed: false,
            owner,
            signaler,
            _slot: slot,
            _unit: unit,
            _bytes: bytes,
            _source: source,
        };
        let entry = self
            .records
            .prepare_insert(context, watch, &libbudget::AllocatorBacked)
            .map_err(|failure| match failure {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(error, _) => {
                    system(libbudget::allocator_error(error))
                }
            })?;
        Ok(PreparedWatch { entry })
    }
    /// 来源/回复/控制输出容量已准入，授权和目标在插入前复核；此处不分配。
    pub fn commit(&mut self, prepared: PreparedWatch) -> WatchInfo {
        let info = prepared.info();
        self.records.insert_prepared(prepared.entry);
        info
    }
    pub fn funding(&self, context: u64) -> Option<&crate::Funding> {
        self.records.get(context).map(|watch| &watch.funding)
    }
    pub fn target(&self, context: u64) -> Option<NodeId> {
        self.records
            .get(context)
            .and_then(|watch| watch.state.target)
    }
    pub fn query(&self, context: u64) -> Result<WatchInfo, Status> {
        self.records
            .get(context)
            .map(|watch| watch.state.sequence.info())
            .ok_or(Status::Permission)
    }
    pub fn ack(&mut self, context: u64, sequence: u64) -> Result<WatchInfo, Status> {
        let watch = self.records.get_mut(context).ok_or(Status::Permission)?;
        let info = watch.state.sequence.ack(sequence)?;
        if info.pending == 0 {
            match watch.owner.take(u64::MAX) {
                Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                Err(error) => return Err(system(error)),
            }
            watch.signaled = 0;
        }
        Ok(info)
    }
    pub fn cancel(&mut self, context: u64) -> Result<WatchInfo, Status> {
        let watch = self.records.get_mut(context).ok_or(Status::Permission)?;
        watch.terminate(WatchReason::Cancelled);
        Ok(watch.state.sequence.info())
    }
    pub fn changed(&mut self, target: NodeId, bits: u64) {
        let mut after = None;
        while let Some((context, watch)) = self.records.next_after(after.as_ref()) {
            let context = *context;
            let selected = watch.state.target == Some(target);
            after = Some(context);
            if selected {
                self.records
                    .get_mut(context)
                    .expect("selected file watch vanished")
                    .change(bits);
            }
        }
    }
    /// 当前单 owner 下检查真实 backing 弱身份，不凭名字重新选择目标。
    pub(crate) fn refresh(&mut self, context: u64, target: NodeState) {
        let Some(watch) = self.records.get_mut(context) else {
            return;
        };
        if watch.state.sequence.info().state == WatchState::Terminal {
            return;
        }
        if watch
            .state
            .authority
            .as_ref()
            .is_some_and(|lease| !lease.active.get())
        {
            watch.terminate(WatchReason::Revoked);
        } else {
            if target != NodeState::Linked && !watch.state.unbound {
                watch.change(WATCH_UNBOUND);
            }
            if target == NodeState::Retiring {
                watch.terminate(WatchReason::TargetRetired);
            }
        }
    }
    pub fn signal(&mut self, context: u64) -> Result<(), SystemCallError> {
        let Some(watch) = self.records.get_mut(context) else {
            return Ok(());
        };
        let info = watch.state.sequence.info();
        if watch.notification_closed || info.pending == 0 || watch.signaled == info.sequence {
            return Ok(());
        }
        match watch.signaler.signal(1) {
            Ok(()) => {
                watch.signaled = info.sequence;
                Ok(())
            }
            Err(SystemCallError::ObjectClosed) => {
                watch.notification_closed = true;
                watch.terminate(WatchReason::Cancelled);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    pub fn next_context(&self, after: Option<u64>) -> Option<u64> {
        self.records.next_after(after.as_ref()).map(|(key, _)| *key)
    }
    pub fn remove(&mut self, context: u64) {
        let _ = self.records.remove(context);
    }
    pub fn seal(&mut self) {
        self.sealed = true;
        let mut after = None;
        while let Some(context) = self.next_context(after) {
            after = Some(context);
            self.records
                .get_mut(context)
                .expect("selected file watch vanished")
                .terminate(WatchReason::ProviderStopping);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{WATCH_CREATE, WATCH_MODIFY};
    #[test]
    fn exhaustion_releases_weak_target_and_authority_permit() {
        use core::cell::Cell;
        use libbudget::{Budget, Taxonomy};
        let mut limits = [0; FsResource::COUNT];
        limits[FsResource::Grant.slot()] = 1;
        limits[FsResource::WaitSource.slot()] = 1;
        limits[FsResource::Bytes.slot()] = core::mem::size_of::<GrantLease>();
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let slots: [_; FsResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
        let account: libbudget::AccountView<FsResource> = account.view(&slots).unwrap();
        let counter = Arc::new(Counter::new(1));
        let lease = Rc::new(GrantLease {
            active: Cell::new(true),
            _slot: Counter::try_acquire(&counter).unwrap(),
            _grant_charge: account.acquire(FsResource::Grant, 1).unwrap(),
            _observer_charge: account.acquire(FsResource::WaitSource, 1).unwrap(),
            _storage_charge: account
                .acquire(FsResource::Bytes, core::mem::size_of::<GrantLease>())
                .unwrap(),
        });
        let mut state = State {
            target: NodeId::from_raw(1),
            authority: Some(lease),
            sequence: Sequence::new(WATCH_MODIFY).unwrap(),
            unbound: false,
        };
        state.sequence.info.sequence = u64::MAX - 1;
        state.sequence.info.pending = 0;
        assert_eq!(account.usage(FsResource::Grant).0, 1);
        state.change(WATCH_MODIFY);
        assert_eq!(state.sequence.info().reason, WatchReason::SequenceExhausted);
        assert!(state.target.is_none());
        assert!(state.authority.is_none());
        assert_eq!(account.usage(FsResource::Grant).0, 0);
        assert_eq!(account.usage(FsResource::WaitSource).0, 0);
        assert_eq!(account.usage(FsResource::Bytes).0, 0);
        assert!(Counter::try_acquire(&counter).is_ok());
    }
    #[test]
    fn stale_ack_keeps_changes_between_query_and_ack() {
        let mut sequence = Sequence::new(WATCH_MODIFY).unwrap();
        let queried = sequence.info().sequence;
        sequence.change(WATCH_MODIFY);
        let fresh = sequence.info();
        assert_eq!(sequence.ack(queried).unwrap(), fresh);
        assert_ne!(fresh.pending & WATCH_MODIFY, 0);
        assert_eq!(sequence.ack(fresh.sequence).unwrap().pending, 0);
        assert_eq!(sequence.ack(fresh.sequence).unwrap().pending, 0);
        assert_eq!(sequence.ack(fresh.sequence + 1), Err(Status::Invalid));
    }
    #[test]
    fn mask_cannot_hide_initial_or_terminal() {
        let mut sequence = Sequence::new(0).unwrap();
        assert_eq!(sequence.info().pending, WATCH_INITIAL);
        sequence.change(WATCH_CREATE);
        assert_eq!(sequence.info().sequence, 1);
        sequence.ack(1).unwrap();
        sequence.terminate(WatchReason::Cancelled);
        assert_eq!(sequence.info().pending, WATCH_TERMINAL);
        let terminal = sequence.info();
        sequence.terminate(WatchReason::ProviderStopping);
        sequence.change(WATCH_MODIFY);
        assert_eq!(sequence.info(), terminal);
        assert_eq!(Sequence::new(WATCH_INITIAL).err(), Some(Status::Invalid));
    }
    #[test]
    fn exhaustion_preserves_one_unacknowledged_terminal_sequence() {
        let mut sequence = Sequence::new(WATCH_MODIFY).unwrap();
        sequence.info.sequence = u64::MAX - 1;
        sequence.info.pending = 0;
        sequence.change(WATCH_MODIFY);
        assert_eq!(sequence.info().sequence, u64::MAX);
        assert_eq!(sequence.info().pending, WATCH_TERMINAL);
        assert_eq!(sequence.info().reason, WatchReason::SequenceExhausted);
        assert_ne!(sequence.ack(u64::MAX - 1).unwrap().pending, 0);
    }
}
