//! DMA mapping 的稳定保留、地址交付屏障与预付退休 owner。
use super::*;
use crate::task::memory_object::MemoryObjectCore;
use crate::vm::backing::{BackingAcquisition, BackingRelease, BackingUse};
use dma::MappingState;
use erhino_shared::dma::{MappingPhase, MappingSnapshot, Segment};
static NEXT_MAPPING: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);
// 只拥有物理责任的强根，不拥有域状态；Root 的成员表只持 Weak，不能形成强环。
static RETAINED: Spinlock<OrderedTable<Arc<Mapping>, MetadataCharge>> =
    Spinlock::new(ranks::LEAF, OrderedTable::new(usize::MAX));
pub(super) struct Member {
    pub(super) mapping: Weak<Mapping>,
    state: MappingState,
    previous: Option<u64>,
    pub(super) next: Option<u64>,
}
#[derive(Default)]
pub(super) struct Owned {
    pub(super) acquisition: Option<BackingAcquisition>,
    pub(super) usage: Option<BackingUse>,
    pub(super) write: Option<(Arc<MemoryObjectCore>, memory_space::WritePermit)>,
    release: Option<BackingRelease>,
}
pub(super) struct Mapping {
    pub(super) owner: Arc<Node>,
    id: u64,
    pub(super) offset: usize,
    pub(super) bytes: usize,
    pub(super) direction: u32,
    pub(super) completion: Arc<SignalSource>,
    pub(super) owned: Spinlock<Option<Owned>>,
    record: WorkRecord,
    dispatch_closed: Spinlock<bool>,
    _metadata: MetadataCharge,
}
pub(super) struct PreparedMapping {
    pub(super) mapping: Arc<Mapping>,
    member: Option<PreparedEntry<Member, MetadataCharge>>,
    retained: Option<PreparedEntry<Arc<Mapping>, MetadataCharge>>,
}
impl PreparedMapping {
    pub(super) fn new(
        owner: Arc<Node>,
        pool: &Arc<MemoryPool>,
        offset: usize,
        bytes: usize,
        direction: u32,
    ) -> Result<Self, Error> {
        let id = NEXT_MAPPING.allocate().ok_or(Error::ReachLimit)?;
        let metadata = MetadataCharge::arc::<Mapping>(pool)?;
        let mapping = Arc::try_new(Mapping {
            owner,
            id,
            offset,
            bytes,
            direction,
            completion: SignalSource::try_new(pool.clone())?,
            owned: Spinlock::new(ranks::LEAF, Some(Owned::default())),
            record: WorkRecord::new(pool.maintenance_account().clone()),
            dispatch_closed: Spinlock::new(ranks::LEAF, false),
            _metadata: metadata,
        })
        .map_err(|_| Error::OutOfMemory)?;
        let member = entry(
            id,
            Member {
                mapping: Arc::downgrade(&mapping),
                state: MappingState::new(),
                previous: None,
                next: None,
            },
            pool,
        )?;
        let retained = entry(id, mapping.clone(), pool)?;
        Ok(Self {
            mapping,
            member: Some(member),
            retained: Some(retained),
        })
    }
    pub(super) fn commit(&mut self, admission: &Admission) -> Result<(), Error> {
        let mapping = &self.mapping;
        {
            let mut state = mapping.owner.root.state.lock();
            if state.tree.validate(admission).map_err(tree_error)? != mapping.owner.id {
                return Err(Error::RightsDenied);
            }
            state.tree.add_member(admission).map_err(tree_error)?;
            let tail = state
                .tree
                .value(mapping.owner.id)
                .expect("DMA owner missing")
                .tail;
            let mut entry = self.member.take().expect("DMA pin committed twice");
            entry.value_mut().previous = tail;
            if let Some(tail) = tail {
                state.mappings.get_mut(tail).expect("DMA tail missing").next = Some(mapping.id);
            } else {
                state
                    .tree
                    .value_mut(mapping.owner.id)
                    .expect("DMA owner missing")
                    .head = Some(mapping.id);
            }
            state
                .tree
                .value_mut(mapping.owner.id)
                .expect("DMA owner missing")
                .tail = Some(mapping.id);
            state.mappings.insert_prepared(entry);
        }
        // 同一有界准备步骤尚未让出，原请求仍强持记录；插入消费预付节点、不会失败。
        RETAINED
            .lock()
            .insert_prepared(self.retained.take().expect("DMA retention committed twice"));
        Ok(())
    }
}
impl Mapping {
    pub(super) fn snapshot(&self) -> MappingSnapshot {
        let state = self.owner.root.state.lock();
        let state = &state.mappings.get(self.id).expect("DMA map missing").state;
        MappingSnapshot {
            bytes: self.bytes as u64,
            phase: state.phase() as u32,
            direction: self.direction,
            readers: state.readers() as u64,
            reserved: 0,
        }
    }
    pub(super) fn stopped(&self) -> bool {
        self.owner
            .root
            .state
            .lock()
            .mappings
            .get(self.id)
            .expect("DMA map missing")
            .state
            .stopped()
    }
    pub(super) fn prepared(self: &Arc<Self>, success: bool) -> bool {
        let ready = self
            .owner
            .root
            .state
            .lock()
            .mappings
            .get_mut(self.id)
            .expect("DMA map missing")
            .state
            .prepared(success);
        if !ready {
            self.notify();
        }
        ready
    }
    pub(super) fn abandon_preparation(self: &Arc<Self>) {
        let changed = {
            let mut state = self.owner.root.state.lock();
            let Some(member) = state.mappings.get_mut(self.id) else {
                return;
            };
            if member.state.phase() != MappingPhase::Preparing {
                return;
            }
            member.state.prepared(false);
            true
        };
        if changed {
            self.notify();
        }
    }
    pub(super) fn stop(self: &Arc<Self>) {
        let release = {
            let mut state = self.owner.root.state.lock();
            let Some(member) = state.mappings.get_mut(self.id) else {
                return;
            };
            member.state.stop();
            member.state.releasing()
        };
        if release {
            self.notify();
        }
    }
    pub(super) fn retire(self: &Arc<Self>) -> Result<(), Error> {
        let release = {
            let mut state = self.owner.root.state.lock();
            let member = state.mappings.get_mut(self.id).expect("DMA map missing");
            member.state.retire()?;
            member.state.releasing()
        };
        if release {
            self.notify();
        }
        Ok(())
    }
    fn notify(self: &Arc<Self>) {
        {
            let closed = self.dispatch_closed.lock();
            if *closed {
                return;
            }
            let work: Arc<dyn Work> = self.clone();
            mt::publish(work);
        }
        mt::kick();
    }
    pub(super) fn expose(self: &Arc<Self>, admission: &Admission) -> Result<Exposure, Error> {
        let mut state = self.owner.root.state.lock();
        if state.tree.validate(admission).map_err(tree_error)? != self.owner.id {
            return Err(Error::RightsDenied);
        }
        state
            .mappings
            .get_mut(self.id)
            .expect("DMA map missing")
            .state
            .expose()?;
        Ok(Exposure {
            mapping: self.clone(),
        })
    }
    fn release_reader(self: &Arc<Self>) {
        let release = {
            let mut state = self.owner.root.state.lock();
            let member = state.mappings.get_mut(self.id).expect("DMA map missing");
            member.state.release_reader();
            member.state.runnable()
        };
        if release {
            self.notify();
        }
    }
    pub(super) fn segments(&self, offset: usize, output: &mut [Segment]) -> usize {
        // 调用者持已经登记的 Exposure，退休不能取走 Owned 或归还 use。
        let owned = self.owned.lock();
        let usage = owned
            .as_ref()
            .expect("DMA reader raced retirement")
            .usage
            .as_ref()
            .expect("ready DMA map lost backing use");
        let mut at = offset;
        for segment in output {
            if at == self.bytes {
                break;
            }
            let logical = self.offset + at;
            let (physical, pages) = usage
                .physical_span((logical / memory_space::PAGE_SIZE) as u64)
                .expect("DMA pin lost physical span");
            let in_page = logical % memory_space::PAGE_SIZE;
            let bytes = (pages * memory_space::PAGE_SIZE - in_page).min(self.bytes - at);
            *segment = Segment {
                address: (physical.0 * memory_space::PAGE_SIZE + in_page) as u64,
                bytes: bytes as u64,
            };
            at += bytes;
        }
        at
    }
}
pub(super) struct Exposure {
    mapping: Arc<Mapping>,
}
impl Drop for Exposure {
    fn drop(&mut self) {
        self.mapping.release_reader();
    }
}
impl Work for Mapping {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        use maintenance::record::Outcome;
        let phase = {
            let state = self.owner.root.state.lock();
            let member = state.mappings.get(self.id).expect("DMA map missing");
            if !member.state.runnable() {
                return Outcome::Idle;
            }
            member.state.phase()
        };
        let mut owned = self
            .owned
            .lock()
            .take()
            .expect("DMA release has another owner");
        let more = if let Some(acquisition) = owned.acquisition.take() {
            owned.usage = acquisition.finish();
            true
        } else if let Some((core, permit)) = owned.write.take() {
            if phase == MappingPhase::Cancelling {
                core.cancel_write(permit);
            } else {
                // SAFETY: 受信 Retire 已保证设备静止；coherent 模式只需排序已完成访问。
                unsafe {
                    core::arch::asm!("fence iorw, iorw", options(nostack));
                }
                core.retire_write(permit);
            }
            true
        } else if let Some(usage) = owned.usage.take() {
            // 只读 DMA 同样在显式退休后执行平台排序，CPU fence 不充当静止证明。
            if phase == MappingPhase::Retiring {
                // SAFETY: S 态执行普通内存/IO 排序，无地址操作。
                unsafe {
                    core::arch::asm!("fence iorw, iorw", options(nostack));
                }
            }
            owned.release = Some(usage.retire());
            true
        } else if let Some(release) = &mut owned.release {
            if release.step() {
                owned.release.take();
            }
            true
        } else {
            false
        };
        *self.owned.lock() = Some(owned);
        if more {
            return Outcome::Ready;
        }
        {
            let mut state = self.owner.root.state.lock();
            let member = state.mappings.get_mut(self.id).expect("DMA map missing");
            member.state.finish();
            let (previous, next) = (member.previous, member.next);
            if let Some(previous) = previous {
                state
                    .mappings
                    .get_mut(previous)
                    .expect("DMA previous member missing")
                    .next = next;
            } else {
                state
                    .tree
                    .value_mut(self.owner.id)
                    .expect("DMA owner missing")
                    .head = next;
            }
            if let Some(next) = next {
                state
                    .mappings
                    .get_mut(next)
                    .expect("DMA next member missing")
                    .previous = previous;
            } else {
                state
                    .tree
                    .value_mut(self.owner.id)
                    .expect("DMA owner missing")
                    .tail = previous;
            }
            let link = state
                .tree
                .value_mut(self.owner.id)
                .expect("DMA owner missing");
            if link.stop_next == Some(self.id) {
                link.stop_next = next;
            }
            state.tree.finish_member(self.owner.id);
        }
        *self.dispatch_closed.lock() = true;
        let retained = RETAINED
            .lock()
            .remove(self.id)
            .expect("DMA retention lost its mapping");
        drop(retained);
        self.completion.publish(ObjectSignals::DONE, false);
        self.owner.notify();
        Outcome::Done
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        let entry = self.owner.root.state.lock().mappings.remove(self.id);
        if let Some(entry) = &entry {
            assert!(
                matches!(
                    entry.value().state.phase(),
                    MappingPhase::Cancelled | MappingPhase::Retired
                ),
                "DMA mapping dropped before retirement"
            );
        }
        drop(entry);
    }
}
