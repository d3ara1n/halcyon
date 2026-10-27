//! 每空间预付的 MMIO 强制退休执行者。FIFO 成员节点在 Map 准入时准备，
//! 执行时只复用固定计划、内联 PTE 游标和既有 ChangeContext。

use super::{
    change::{ChangeContext, Phase, VmAccessLease, VmShell},
    remote::Transport,
    space::{ChangeProgress, RetireBatch},
    views::Views,
};
use crate::resource::mmio::MmioMapping;
use crate::{
    maintenance::{self as mt, WakeToken, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::{MetadataCharge, MetadataFunding, map_storage_error},
        wait_source::{Hint, HintTarget, ObservationSource, WaitNode},
    },
};
use alloc::sync::{Arc, Weak};
use core::pin::Pin;
use erhino_shared::{call::SystemCallError, object::ObjectSignals};
use maintenance::record::Outcome;
use memory_space::{MmioExecutorState, MmioIdle, MmioNotify};
use ordered_table::{OrderedTable, PreparedEntry};

pub(crate) struct QueueEntry {
    pub(crate) mapping: Option<Arc<MmioMapping>>,
    next: Option<u64>,
}
pub(crate) type QueueNode = PreparedEntry<QueueEntry, MetadataCharge>;
pub(crate) fn prepare_node(id: u64, pool: &Arc<MemoryPool>) -> Result<QueueNode, SystemCallError> {
    PreparedEntry::try_new(
        id,
        QueueEntry {
            mapping: None,
            next: None,
        },
        &MetadataFunding(pool),
    )
    .map_err(|error| match error {
        ordered_table::InsertError::Storage(error, _) => map_storage_error(error),
        ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
    })
}
struct Queue {
    entries: OrderedTable<QueueEntry, MetadataCharge>,
    head: Option<u64>,
    tail: Option<u64>,
    dispatch: MmioExecutorState,
    wake: Option<WakeToken>,
}
struct Active {
    mapping: Arc<MmioMapping>,
    _lease: VmAccessLease,
    frozen: bool,
}
struct Dependency {
    source: Arc<dyn ObservationSource>,
    attached: bool,
}
struct Execution {
    active: Option<Active>,
    waiting: Option<Dependency>,
    phase: Phase,
    tables: RetireBatch,
    storage: memory_space::ChangeStorage<MetadataCharge>,
}
pub(crate) struct ForcedRetirement {
    record: WorkRecord,
    vm: Weak<VmShell>,
    views: Arc<Views>,
    queue: Spinlock<Queue>,
    execution: Spinlock<Option<Execution>>,
    transport: Transport,
    node: WaitNode,
    _metadata: MetadataCharge,
    // 末字段：所有宿主存储/费用退出后，才允许整空间退休越过访问租约屏障。
    exit: Spinlock<Option<VmAccessLease>>,
}
impl ForcedRetirement {
    pub(super) fn new(
        pool: &Arc<MemoryPool>,
        vm: Weak<VmShell>,
        views: Arc<Views>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        let mut storage = memory_space::ChangeStorage::default();
        storage
            .prepare_forced_remove(&MetadataFunding(pool))
            .map_err(map_storage_error)?;
        Arc::try_new(Self {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            vm,
            views,
            queue: Spinlock::new(
                ranks::LEAF,
                Queue {
                    entries: OrderedTable::new(usize::MAX),
                    head: None,
                    tail: None,
                    dispatch: MmioExecutorState::new(),
                    wake: None,
                },
            ),
            execution: Spinlock::new(
                ranks::LEAF,
                Some(Execution {
                    active: None,
                    waiting: None,
                    phase: Phase::Idle,
                    tables: RetireBatch::new(),
                    storage,
                }),
            ),
            transport: Transport::new(),
            node: WaitNode::new(),
            _metadata: metadata,
            exit: Spinlock::new(ranks::LEAF, None),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    /// 与队列/通知状态同门提交维护动作；返回的潜在最后根必须带出门外释放。
    fn notify(self: &Arc<Self>, queue: &mut Queue, action: MmioNotify) -> Option<Arc<dyn Work>> {
        match action {
            MmioNotify::Activate => {
                let root: Arc<dyn Work> = self.clone();
                mt::publish(root);
                None
            }
            MmioNotify::WakeDependency => mt::wake(
                queue
                    .wake
                    .take()
                    .expect("MMIO dependency lost its wake token"),
            ),
            MmioNotify::None => None,
        }
    }
    pub(crate) fn enqueue(self: &Arc<Self>, entry: QueueNode) {
        let id = entry.key();
        let (retired, action) = {
            let mut queue = self.queue.lock();
            let Some(action) = queue.dispatch.submit() else {
                return;
            };
            assert!(
                entry.value().mapping.is_some(),
                "MMIO queue node has no owner"
            );
            if let Some(tail) = queue.tail {
                queue
                    .entries
                    .get_mut(tail)
                    .expect("MMIO queue lost tail")
                    .next = Some(id);
            } else {
                queue.head = Some(id);
            }
            queue.tail = Some(id);
            queue.entries.insert_prepared(entry);
            (self.notify(&mut queue, action), action)
        };
        drop(retired);
        if action != MmioNotify::None {
            mt::kick();
        }
    }
    pub(super) fn stop(self: &Arc<Self>, lease: VmAccessLease) {
        assert!(
            self.exit.lock().replace(lease).is_none(),
            "MMIO retirement closed twice"
        );
        // closing 是交接提交点：退出租约已安装，维护通知也必须在该门内提交。
        let (retired, action) = {
            let mut queue = self.queue.lock();
            let action = queue.dispatch.close();
            (self.notify(&mut queue, action), action)
        };
        drop(retired);
        if action != MmioNotify::None {
            mt::kick();
        }
    }
    fn pop(&self) -> Option<Arc<MmioMapping>> {
        let mut entry = {
            let mut queue = self.queue.lock();
            let head = queue.head?;
            let entry = queue.entries.remove(head).expect("MMIO queue lost head");
            queue.head = entry.value().next;
            if queue.head.is_none() {
                queue.tail = None;
            }
            entry
        };
        entry.value_mut().mapping.take()
    }
    fn detach(&self, execution: &mut Execution) {
        if let Some(dependency) = execution.waiting.take()
            && dependency.attached
        {
            // SAFETY: 唯一执行 owner 摘除稳定节点，强来源保留至 detach 返回。
            unsafe {
                dependency.source.detach(Pin::new_unchecked(&self.node));
            }
        }
        let wake = {
            let mut queue = self.queue.lock();
            queue.dispatch.detach_dependency();
            queue.wake.take()
        };
        drop(wake);
    }
    fn poll_dependency(self: &Arc<Self>, execution: &mut Execution, root: &Arc<dyn Work>) -> bool {
        let Some(dependency) = execution.waiting.as_mut() else {
            return true;
        };
        let armed = {
            let mut queue = self.queue.lock();
            if queue.dispatch.arm_dependency() {
                assert!(queue.wake.is_none(), "MMIO dependency armed twice");
                queue.wake = Some(mt::arm(root));
                true
            } else {
                false
            }
        };
        if !armed {
            self.detach(execution);
            return true;
        }
        if !dependency.attached {
            let target: Arc<dyn HintTarget> = self.clone();
            // SAFETY: 节点内嵌于稳定 Work，来源/Work 都保活至唯一 detach。
            unsafe {
                dependency.source.attach(
                    Pin::new_unchecked(&self.node),
                    ObjectSignals::DONE,
                    Hint::new(Arc::downgrade(&target), 0),
                );
            }
            dependency.attached = true;
        }
        if !dependency
            .source
            .snapshot()
            .signals
            .contains(ObjectSignals::DONE)
        {
            return false;
        }
        self.detach(execution);
        true
    }
    fn wait_on(execution: &mut Execution, source: Arc<dyn ObservationSource>) {
        assert!(
            execution.waiting.is_none(),
            "MMIO retirement already waits on a dependency"
        );
        execution.waiting = Some(Dependency {
            source,
            attached: false,
        });
    }
    fn advance(self: &Arc<Self>, execution: &mut Execution, root: &Arc<dyn Work>) -> Outcome {
        let Some(vm) = self.vm.upgrade() else {
            assert!(
                execution.active.is_none(),
                "active MMIO retirement lost its VM lease"
            );
            self.detach(execution);
            return if self.pop().is_some() {
                Outcome::Ready
            } else {
                Outcome::Idle
            };
        };
        if !matches!(execution.phase, Phase::Idle) {
            let context = ChangeContext {
                shell: &vm,
                views: &self.views,
                transport: &self.transport,
            };
            let outcome = context.advance(&mut execution.phase, &mut execution.tables, root);
            if outcome == Outcome::Done {
                let Phase::Done(storage) = core::mem::replace(&mut execution.phase, Phase::Idle)
                else {
                    panic!("forced retirement did not return its storage");
                };
                execution.storage = storage.expect("forced retirement lost prepaid plan");
                return Outcome::Ready;
            }
            return outcome;
        }
        // Frozen 后不启动新单映射事务；已提交的 engine 在上分支完整结束。
        if !vm.space.lock().access_open {
            self.detach(execution);
            if let Some(active) = execution.active.take() {
                if active.frozen {
                    vm.space
                        .lock()
                        .space
                        .as_mut()
                        .expect("active lease lost address space")
                        .ledger_mut()
                        .finish_mapping_retirement(active.mapping.id())
                        .expect("frozen MMIO source retained a forced transaction");
                }
                drop(active);
            } else {
                drop(self.pop());
            }
            return if self.queue.lock().head.is_some() {
                Outcome::Ready
            } else {
                Outcome::Idle
            };
        }
        if !self.poll_dependency(execution, root) {
            return Outcome::Blocked;
        }
        if execution.active.is_none() {
            let Some(mapping) = self.pop() else {
                return Outcome::Idle;
            };
            let Some(lease) = vm.admit_access() else {
                return Outcome::Ready;
            };
            execution.active = Some(Active {
                mapping,
                _lease: lease,
                frozen: false,
            });
        }
        let active = execution
            .active
            .as_mut()
            .expect("MMIO retirement lost active owner");
        let mapping = active.mapping.clone();
        if !active.frozen {
            let mut gate = vm.space.lock();
            if !gate.access_open {
                return Outcome::Ready;
            }
            gate.space
                .as_mut()
                .expect("MMIO retirement lost address space")
                .ledger_mut()
                .freeze_mapping(mapping.id())
                .expect("two forced MMIO owners in one VM");
            active.frozen = true;
        }
        let operation = mapping.operation_completion();
        if !operation.snapshot().signals.contains(ObjectSignals::DONE) {
            Self::wait_on(execution, operation);
            return Outcome::Ready;
        }
        let mut gate = vm.space.lock();
        if !gate.access_open {
            return Outcome::Ready;
        }
        let space = gate
            .space
            .as_mut()
            .expect("MMIO retirement lost address space");
        let Some(address) = self.views.mmio_address(mapping.id()) else {
            drop(gate);
            let source = mapping.completion();
            if !source.snapshot().signals.contains(ObjectSignals::DONE) {
                Self::wait_on(execution, source);
                return Outcome::Ready;
            }
            vm.space
                .lock()
                .space
                .as_mut()
                .expect("active lease lost address space")
                .ledger_mut()
                .finish_mapping_retirement(mapping.id())
                .expect("MMIO source completed with an active transaction");
            drop(execution.active.take());
            return Outcome::Ready;
        };
        if let Some(key) = space.ledger().change_at(address) {
            let source = gate
                .dependencies
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .expect("MMIO retirement lost change dependency")
                .1
                .clone();
            drop(gate);
            Self::wait_on(execution, source);
            return Outcome::Ready;
        }
        let prepared = space
            .ledger_mut()
            .reserve_forced_remove(mapping.id(), address, &mut execution.storage)
            .expect("frozen MMIO fragment invalidated or prepaid storage exhausted");
        let key = prepared.key();
        let footprint = space.ledger().prepared_footprint(&prepared);
        let committed = space.ledger_mut().commit(prepared);
        gate.dependencies.push((key, mapping.completion()));
        execution.phase = Phase::Publishing(ChangeProgress::forced_remove(committed, footprint));
        Outcome::Ready
    }
}
impl Work for ForcedRetirement {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> Outcome {
        self.queue.lock().dispatch.begin_step();
        let mut execution = self
            .execution
            .lock()
            .take()
            .expect("MMIO retirement stepped twice");
        let root: Arc<dyn Work> = self.clone();
        let outcome = self.advance(&mut execution, &root);
        let outcome = {
            let mut queue = self.queue.lock();
            match outcome {
                Outcome::Idle => {
                    let has_queued = queue.head.is_some();
                    match queue.dispatch.finish_idle(has_queued) {
                        MmioIdle::Idle => Outcome::Idle,
                        MmioIdle::Ready => Outcome::Ready,
                        MmioIdle::Done => Outcome::Done,
                    }
                }
                Outcome::Ready => {
                    queue.dispatch.ready();
                    Outcome::Ready
                }
                Outcome::Blocked => {
                    if execution.waiting.is_none() {
                        queue.dispatch.wait_fence();
                    }
                    Outcome::Blocked
                }
                Outcome::Done => unreachable!("MMIO completion bypassed close handoff"),
            }
        };
        let mask = if outcome == Outcome::Blocked && execution.waiting.is_none() {
            self.transport.pending()
        } else {
            0
        };
        *self.execution.lock() = Some(execution);
        if mask != 0 {
            super::remote::kick(mask);
        }
        outcome
    }
}
impl HintTarget for ForcedRetirement {
    fn hint(&self, _identity: u64) {
        // 旧提示至多造成一次多余复检，实际完成只由当前来源 snapshot 决定。
        let (retired, notified) = {
            let mut queue = self.queue.lock();
            if queue.dispatch.dependency_hint() {
                let wake = queue
                    .wake
                    .take()
                    .expect("MMIO dependency hint lost its token");
                (mt::wake(wake), true)
            } else {
                (None, false)
            }
        };
        drop(retired);
        if notified {
            mt::kick();
        }
    }
}
impl Drop for ForcedRetirement {
    fn drop(&mut self) {
        assert!(
            self.queue.get_mut().entries.is_empty(),
            "MMIO queue dropped before draining"
        );
        let execution = self
            .execution
            .get_mut()
            .as_ref()
            .expect("MMIO executor dropped during step");
        assert!(
            execution.active.is_none()
                && execution.waiting.is_none()
                && matches!(execution.phase, Phase::Idle)
                && execution.tables.is_empty()
                && self.transport.idle(),
            "MMIO executor dropped with outstanding responsibility"
        );
    }
}
