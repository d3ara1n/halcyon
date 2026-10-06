//! Body 私有线程成员表；终因与运行准入在 Shell，空间活跃 hart 在 VM。

use alloc::sync::Arc;

use erhino_shared::proc::{PROCESS_MAX_THREADS, Tid};

use crate::sync::{Spinlock, ranks};

use super::{
    super::Thread,
    shell::{ProcessShell, ReadyIngress},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemberKey {
    slot: usize,
    generation: u64,
    tid: Tid,
}

impl MemberKey {
    pub(crate) const fn tid(self) -> Tid {
        self.tid
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachFault {
    Limit,
    CapacityChanged,
}

pub(crate) type MemberStorage =
    funded_storage::Buffer<Slot, crate::task::resources::MetadataCharge>;

pub(crate) enum MemberState {
    Reserved,
    Created(Arc<Thread>),
    Ready,
    Running { slot: usize },
    Waiting(crate::sched::AdmittedThread),
    Exiting,
}

struct Entry {
    tid: Tid,
    state: MemberState,
    /// 不回绕的每线程等待代次；pending 与容器迁移共用本成员锁。
    epoch: u64,
    active_wait: bool,
    pending: bool,
}

pub(crate) struct Slot {
    generation: u64,
    retired: bool,
    entry: Option<Entry>,
}

struct Inner {
    slots: MemberStorage,
    count: usize,
    next_tid: Tid,
}

pub(crate) struct Members {
    inner: Spinlock<Inner>,
}

impl Drop for Members {
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        if inner.count == 0 {
            // count 包含 Reserved 及全部执行状态；归零时每个 entry 都为 None，
            // 无 Thread/AdmittedThread 要销毁，直接释放已付保留槽分配。
            inner.slots.forget_values();
        }
    }
}

pub(crate) enum TerminationSlot {
    Vacant,
    Created(Arc<Thread>),
    Waiting(crate::sched::AdmittedThread),
    Ready,
    Running,
    Exiting,
}

pub(crate) enum WakeTransition {
    Publish(ReadyIngress),
    Cancelled,
}

pub(crate) enum ParkTransition {
    Parked,
    Publish(crate::sched::AdmittedThread, ReadyIngress),
    Exit(crate::sched::AdmittedThread),
}

pub(crate) enum HintTransition {
    Ignored,
    Publish(crate::sched::AdmittedThread, ReadyIngress),
    Exit(crate::sched::AdmittedThread),
}

impl Members {
    pub(crate) fn new() -> Self {
        Self {
            inner: Spinlock::new(
                ranks::PROCESS_MEMBERS,
                Inner {
                    slots: MemberStorage::new(),
                    count: 0,
                    next_tid: 1,
                },
            ),
        }
    }

    /// 仅在发生容量竞争后准备严格大于快照容量的块；下一次失败证明容量已增长。
    pub(crate) fn prepare_storage(
        &self,
        pool: &Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<MemberStorage, erhino_shared::call::SystemCallError> {
        let capacity = {
            let inner = self.inner.lock();
            inner
                .slots
                .capacity()
                .saturating_mul(2)
                .max(inner.slots.len() + 1)
        };
        MemberStorage::try_empty(capacity, &crate::task::resources::MetadataFunding(pool)).map_err(
            |error| match error {
                funded_storage::StorageError::Funding(error) => error,
                funded_storage::StorageError::Allocation => {
                    erhino_shared::call::SystemCallError::OutOfMemory
                }
                funded_storage::StorageError::Layout => {
                    erhino_shared::call::SystemCallError::ReachLimit
                }
            },
        )
    }

    /// prepared/旧块的 owner 由调用者带出全部祖先 Job 及 Shell 锁后释放。
    pub(crate) fn reserve(&self, prepared: &mut MemberStorage) -> Result<MemberKey, AttachFault> {
        let mut inner = self.inner.lock();
        if inner.count >= PROCESS_MAX_THREADS {
            return Err(AttachFault::Limit);
        }
        let slot = inner
            .slots
            .iter()
            .position(|slot| slot.entry.is_none() && !slot.retired)
            .unwrap_or(inner.slots.len());
        if slot == inner.slots.len() {
            if inner.slots.len() == inner.slots.capacity() {
                if prepared.capacity() <= inner.slots.len() {
                    return Err(AttachFault::CapacityChanged);
                }
                inner
                    .slots
                    .replace_storage(prepared)
                    .expect("prepared member slots fit current data");
            }
            inner.slots.push(Slot {
                generation: 1,
                retired: false,
                entry: None,
            });
        }
        let tid = inner.next_tid;
        inner.next_tid = tid.checked_add(1).ok_or(AttachFault::Limit)?;
        let generation = inner.slots[slot].generation;
        assert!(
            inner.slots[slot]
                .entry
                .replace(Entry {
                    tid,
                    state: MemberState::Reserved,
                    epoch: 0,
                    active_wait: false,
                    pending: false,
                })
                .is_none()
        );
        inner.count += 1;
        Ok(MemberKey {
            slot,
            generation,
            tid,
        })
    }

    /// Job 链闸门通过后才使线程成为真实成员；本段不分配。
    pub(crate) fn commit(&self, key: MemberKey, state: MemberState) {
        assert!(
            !matches!(&state, MemberState::Reserved),
            "reserved thread committed as reserved"
        );
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        assert!(
            matches!(&entry.state, MemberState::Reserved),
            "thread committed twice"
        );
        entry.state = state;
    }

    pub(crate) fn rollback(&self, key: MemberKey) {
        let removed = self.inner.lock().remove(key);
        assert!(
            matches!(&removed, MemberState::Reserved),
            "published thread rolled back"
        );
    }

    /// Created 控制权交队列前提取线程，后续由类队列持 admission 强引用。
    pub(crate) fn start_created(&self, key: MemberKey) -> Option<Arc<Thread>> {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        if !matches!(&entry.state, MemberState::Created(_)) {
            return None;
        }
        match core::mem::replace(&mut entry.state, MemberState::Ready) {
            MemberState::Created(thread) => Some(thread),
            _ => unreachable!(),
        }
    }

    /// 最后 Control 放弃 Created 启动票据；与 Start/终止扫描同锁仲裁。
    pub(crate) fn claim_created_departure(&self, key: MemberKey) -> Option<Arc<Thread>> {
        let mut inner = self.inner.lock();
        // 外部 Pending 票据的存储可长于成员；终止扫描/旧回执已摘除时幂等落败。
        let slot = inner.slots.get_mut(key.slot)?;
        if slot.generation != key.generation {
            return None;
        }
        let entry = slot.entry.as_mut()?;
        if entry.tid != key.tid {
            return None;
        }
        if !matches!(&entry.state, MemberState::Created(_)) {
            return None;
        }
        match core::mem::replace(&mut entry.state, MemberState::Exiting) {
            MemberState::Created(thread) => Some(thread),
            _ => unreachable!(),
        }
    }

    /// ProcessStart 在外部 ingress lease 下逐槽兑现已准入的预建线程。
    /// 首个 Ready 发布前冻结出生批次；槽复用不能把 Running 期新票据混入启动。
    pub(crate) fn start_snapshot(&self) -> (usize, Tid) {
        let inner = self.inner.lock();
        (inner.slots.len(), inner.next_tid)
    }

    pub(crate) fn start_created_at(&self, index: usize, tid_ceiling: Tid) -> Option<Arc<Thread>> {
        let mut inner = self.inner.lock();
        let entry = inner.slots.get_mut(index)?.entry.as_mut()?;
        if entry.tid >= tid_ceiling || !matches!(&entry.state, MemberState::Created(_)) {
            return None;
        }
        match core::mem::replace(&mut entry.state, MemberState::Ready) {
            MemberState::Created(thread) => Some(thread),
            _ => unreachable!(),
        }
    }

    /// 调度类摘 Ready 后到 VM 门登记前，仅修改成员容器真值。
    pub(crate) fn enter_running(&self, key: MemberKey, slot: usize) -> bool {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        if !matches!(&entry.state, MemberState::Ready) {
            return false;
        }
        entry.state = MemberState::Running { slot };
        true
    }

    pub(crate) fn requeue(
        &self,
        shell: &Arc<ProcessShell>,
        key: MemberKey,
        slot: usize,
    ) -> WakeTransition {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        if !matches!(&entry.state, MemberState::Running { slot: active } if *active == slot) {
            return WakeTransition::Cancelled;
        }
        match shell.try_publish_ready() {
            Some(ingress) => {
                entry.state = MemberState::Ready;
                WakeTransition::Publish(ingress)
            }
            None => {
                entry.state = MemberState::Exiting;
                WakeTransition::Cancelled
            }
        }
    }

    /// 安装者仍持唯一执行 owner；提示不能在节点准备过程中取走它。
    pub(crate) fn begin_wait(&self, key: MemberKey, slot: usize) -> Option<u64> {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        if !matches!(&entry.state, MemberState::Running { slot: active } if *active == slot) {
            return None;
        }
        assert!(!entry.active_wait, "thread started two wait rounds");
        entry.epoch = entry.epoch.checked_add(1)?;
        entry.active_wait = true;
        entry.pending = false;
        Some(entry.epoch)
    }

    /// 复检前清 pending；来源/期限之后到达的提示在最终 park 同锁复检。
    pub(crate) fn take_pending(&self, key: MemberKey, epoch: u64) {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        assert!(
            entry.active_wait && entry.epoch == epoch,
            "wait epoch changed under owner"
        );
        entry.pending = false;
    }

    /// 所有节点 detach 之后关闭本轮；在途旧消息只能幂等落败。
    pub(crate) fn end_wait(&self, key: MemberKey, epoch: u64) {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(key);
        assert!(
            entry.active_wait && entry.epoch == epoch,
            "detached wait lost epoch"
        );
        entry.active_wait = false;
        entry.pending = false;
    }

    pub(crate) fn park(
        &self,
        shell: &Arc<ProcessShell>,
        owner: crate::sched::AdmittedThread,
        slot: usize,
        epoch: u64,
    ) -> ParkTransition {
        let mut inner = self.inner.lock();
        let entry = inner.entry_mut(owner.member());
        assert!(
            entry.active_wait && entry.epoch == epoch,
            "park lost wait epoch"
        );
        if !matches!(&entry.state, MemberState::Running { slot: active } if *active == slot)
            || shell.is_frozen()
        {
            entry.state = MemberState::Exiting;
            return ParkTransition::Exit(owner);
        }
        if entry.pending {
            return match shell.try_publish_ready() {
                Some(ingress) => {
                    entry.state = MemberState::Ready;
                    ParkTransition::Publish(owner, ingress)
                }
                None => {
                    entry.state = MemberState::Exiting;
                    ParkTransition::Exit(owner)
                }
            };
        }
        entry.state = MemberState::Waiting(owner);
        ParkTransition::Parked
    }

    /// 提示与代次/容器同锁仲裁，不接触现场、来源节点或 VM。
    pub(crate) fn hint(
        &self,
        shell: &Arc<ProcessShell>,
        key: MemberKey,
        epoch: u64,
    ) -> HintTransition {
        let mut inner = self.inner.lock();
        let Some(entry) = inner.get_mut(key) else {
            return HintTransition::Ignored;
        };
        if !entry.active_wait || entry.epoch != epoch {
            return HintTransition::Ignored;
        }
        entry.pending = true;
        match &entry.state {
            MemberState::Exiting => return HintTransition::Ignored,
            MemberState::Waiting(_) => {}
            _ => return HintTransition::Ignored,
        }
        let ingress = shell.try_publish_ready();
        let next = if ingress.is_some() {
            MemberState::Ready
        } else {
            MemberState::Exiting
        };
        let MemberState::Waiting(owner) = core::mem::replace(&mut entry.state, next) else {
            unreachable!("wait owner disappeared under member lock");
        };
        match ingress {
            Some(ingress) => HintTransition::Publish(owner, ingress),
            None => HintTransition::Exit(owner),
        }
    }

    pub(crate) fn depart(&self, key: MemberKey) {
        let removed = self.inner.lock().remove(key);
        assert!(
            !matches!(&removed, MemberState::Reserved),
            "uncommitted thread departed"
        );
        drop(removed);
    }

    pub(crate) fn slot_count(&self) -> usize {
        self.inner.lock().slots.len()
    }

    /// Frozen 后每次只拆一个稳定槽；调用者在锁外取消/析构对象责任。
    pub(crate) fn termination_step(&self, index: usize) -> TerminationSlot {
        let mut inner = self.inner.lock();
        let Some(slot) = inner.slots.get_mut(index) else {
            return TerminationSlot::Vacant;
        };
        let Some(entry) = slot.entry.as_mut() else {
            return TerminationSlot::Vacant;
        };
        let old = core::mem::replace(&mut entry.state, MemberState::Exiting);
        match old {
            MemberState::Reserved => {
                entry.state = MemberState::Reserved;
                TerminationSlot::Vacant
            }
            MemberState::Created(thread) => TerminationSlot::Created(thread),
            MemberState::Ready => TerminationSlot::Ready,
            MemberState::Running { .. } => TerminationSlot::Running,
            MemberState::Waiting(context) => TerminationSlot::Waiting(context),
            MemberState::Exiting => TerminationSlot::Exiting,
        }
    }
}

impl Inner {
    fn get_mut(&mut self, key: MemberKey) -> Option<&mut Entry> {
        let slot = self.slots.get_mut(key.slot)?;
        if slot.generation != key.generation {
            return None;
        }
        slot.entry.as_mut().filter(|entry| entry.tid == key.tid)
    }
    fn entry_mut(&mut self, key: MemberKey) -> &mut Entry {
        let slot = self
            .slots
            .get_mut(key.slot)
            .expect("member slot escaped Body");
        assert_eq!(slot.generation, key.generation, "stale member generation");
        let entry = slot.entry.as_mut().expect("member slot retired");
        assert_eq!(entry.tid, key.tid, "member identity changed");
        entry
    }

    fn remove(&mut self, key: MemberKey) -> MemberState {
        let slot = self
            .slots
            .get_mut(key.slot)
            .expect("member slot escaped Body");
        assert_eq!(slot.generation, key.generation, "stale member generation");
        let entry = slot.entry.take().expect("member removed twice");
        assert_eq!(entry.tid, key.tid, "member identity changed");
        if slot.generation == u64::MAX {
            slot.retired = true;
        } else {
            slot.generation += 1;
        }
        self.count -= 1;
        entry.state
    }
}
