//! Provider-local Watch publication table; signaler ownership remains in Runtime tasks.

use alloc::vec::Vec;
use libfal::{
    protocol::{SubscriptionInfo, WatchMask, WatchReason},
    store::NodeId,
};

pub const LIMIT: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct Effect {
    pub node: NodeId,
    pub generation: u64,
    pub events: WatchMask,
    pub terminal: Option<WatchReason>,
}

#[derive(Default)]
pub struct Effects {
    items: [Option<Effect>; 3],
    len: usize,
}

impl Effects {
    pub fn push(&mut self, effect: Effect) {
        assert!(
            self.len < self.items.len(),
            "Watch effect capacity exceeded"
        );
        self.items[self.len] = Some(effect);
        self.len += 1;
    }

    fn iter(&self) -> impl Iterator<Item = Effect> + '_ {
        self.items[..self.len].iter().copied().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlError {
    NotFound,
    Permission,
}

struct Record {
    id: u64,
    context: u64,
    node: NodeId,
    task: u64,
    generation: u64,
    mask: WatchMask,
    pending: WatchMask,
    reason: WatchReason,
}

pub struct WakeSet {
    tasks: [u64; LIMIT],
    len: usize,
}

impl WakeSet {
    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.tasks[..self.len].iter().copied()
    }
}

pub struct Table {
    records: Vec<Record>,
    next_id: u64,
}

impl Table {
    pub fn new() -> Result<Self, alloc::collections::TryReserveError> {
        let mut records = Vec::new();
        records.try_reserve_exact(LIMIT)?;
        Ok(Self {
            records,
            next_id: 1,
        })
    }

    pub fn allocate_id(&mut self) -> Option<u64> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1)?;
        Some(id)
    }

    pub fn install(
        &mut self,
        id: u64,
        context: u64,
        node: NodeId,
        task: u64,
        generation: u64,
        requested: WatchMask,
    ) -> SubscriptionInfo {
        assert!(
            self.records.len() < self.records.capacity(),
            "Watch table overflow"
        );
        let mask = requested | WatchMask::TERMINATED;
        self.records.push(Record {
            id,
            context,
            node,
            task,
            generation,
            mask,
            pending: WatchMask::NONE,
            reason: WatchReason::Active,
        });
        SubscriptionInfo {
            id,
            generation,
            effective_mask: mask,
            pending: WatchMask::NONE,
            reason: WatchReason::Active,
        }
    }

    pub fn query(&self, id: u64, context: u64) -> Result<SubscriptionInfo, ControlError> {
        let record = self
            .records
            .iter()
            .find(|record| record.id == id)
            .ok_or(ControlError::NotFound)?;
        if record.context != context {
            return Err(ControlError::Permission);
        }
        Ok(Self::info(record))
    }

    pub fn cancel(&mut self, id: u64, context: u64) -> Result<u64, ControlError> {
        let index = self
            .records
            .iter()
            .position(|record| record.id == id)
            .ok_or(ControlError::NotFound)?;
        if self.records[index].context != context {
            return Err(ControlError::Permission);
        }
        Ok(self.records.swap_remove(index).task)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(index) = self.records.iter().position(|record| record.id == id) else {
            return false;
        };
        self.records.swap_remove(index);
        true
    }

    pub fn contains(&self, id: u64) -> bool {
        self.records.iter().any(|record| record.id == id)
    }

    pub fn take_pending(&mut self, id: u64) -> Option<(WatchMask, WatchReason)> {
        let record = self.records.iter_mut().find(|record| record.id == id)?;
        let pending = core::mem::replace(&mut record.pending, WatchMask::NONE);
        Some((pending, record.reason))
    }

    pub fn publish(&mut self, effects: &Effects) -> WakeSet {
        let mut wakes = WakeSet {
            tasks: [0; LIMIT],
            len: 0,
        };
        for effect in effects.iter() {
            for record in &mut self.records {
                if record.node != effect.node || record.reason != WatchReason::Active {
                    continue;
                }
                let mut events = effect.events.intersect(record.mask);
                if let Some(reason) = effect.terminal {
                    record.reason = reason;
                    events |= WatchMask::TERMINATED;
                }
                if events.is_empty() {
                    continue;
                }
                let was_empty = record.pending.is_empty();
                record.pending |= events;
                record.generation = effect.generation;
                if was_empty && !wakes.tasks[..wakes.len].contains(&record.task) {
                    wakes.tasks[wakes.len] = record.task;
                    wakes.len += 1;
                }
            }
        }
        wakes
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn info(record: &Record) -> SubscriptionInfo {
        SubscriptionInfo {
            id: record.id,
            generation: record.generation,
            effective_mask: record.mask,
            pending: record.pending,
            reason: record.reason,
        }
    }
}
