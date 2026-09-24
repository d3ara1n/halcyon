//! Provider 通用 Watch 发布表；订阅任务拥有 signaler、来源与回复责任。

use crate::{
    protocol::{SubscriptionInfo, WatchMask, WatchReason},
    store::NodeId,
};
use alloc::vec::Vec;
use erhino_shared::call::SystemCallError;
use libexecution::runtime::Requests;

pub const MAX_MUTATION_EFFECTS: usize = 3;

#[derive(Debug, Clone, Copy)]
pub struct Effect {
    pub node: NodeId,
    pub generation: u64,
    pub events: WatchMask,
    pub terminal: Option<WatchReason>,
}

#[derive(Default)]
pub struct Effects {
    items: [Option<Effect>; MAX_MUTATION_EFFECTS],
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

    pub fn iter(&self) -> impl Iterator<Item = Effect> + '_ {
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

/// 一次提交已经产生、但尚未全部交给 Runtime 的唤醒责任。
///
/// 存储在业务提交前按 provider 的 Watch 准入上界预备；`publish` 只写入已有
/// 容量。Runtime 的单步 Requests 队列满时保留游标，由原任务后续继续兑现。
pub struct WakeBatch {
    tasks: Vec<u64>,
    cursor: usize,
}

impl WakeBatch {
    pub fn new(limit: usize) -> Result<Self, alloc::collections::TryReserveError> {
        let mut tasks = Vec::new();
        tasks.try_reserve_exact(limit)?;
        Ok(Self { tasks, cursor: 0 })
    }

    pub fn is_empty(&self) -> bool {
        self.cursor == self.tasks.len()
    }

    pub fn remaining(&self) -> usize {
        self.tasks.len().saturating_sub(self.cursor)
    }

    pub fn drive<F>(&mut self, requests: &mut Requests<F>) -> Result<bool, SystemCallError> {
        while let Some(task) = self.tasks.get(self.cursor).copied() {
            match requests.wake(task) {
                Ok(()) => self.cursor += 1,
                Err(SystemCallError::ReachLimit) => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        self.tasks.clear();
        self.cursor = 0;
        Ok(true)
    }

    fn begin(&mut self) {
        assert!(
            self.is_empty(),
            "WakeBatch reused with undelivered wake debt"
        );
        self.tasks.clear();
        self.cursor = 0;
    }

    pub fn push_unique(&mut self, task: u64) {
        if !self.tasks.contains(&task) {
            assert!(
                self.tasks.len() < self.tasks.capacity(),
                "WakeBatch capacity is smaller than Watch admission"
            );
            self.tasks.push(task);
        }
    }
}

pub struct Table {
    records: Vec<Record>,
    limit: usize,
    next_id: u64,
}

impl Table {
    pub fn new(limit: usize) -> Result<Self, alloc::collections::TryReserveError> {
        let mut records = Vec::new();
        records.try_reserve_exact(limit)?;
        Ok(Self {
            records,
            limit,
            next_id: 1,
        })
    }

    pub fn limit(&self) -> usize {
        self.limit
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
        assert!(self.records.len() < self.limit, "Watch table overflow");
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

    pub fn publish(&mut self, effects: &Effects, wakes: &mut WakeBatch) {
        wakes.begin();
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
                if was_empty {
                    wakes.push_unique(record.task);
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_batch_retains_more_tasks_than_one_runtime_step() {
        let node = NodeId::from_raw(7).unwrap();
        let mut table = Table::new(24).unwrap();
        for task in 1..=24 {
            let id = table.allocate_id().unwrap();
            table.install(id, 9, node, task, 1, WatchMask::MODIFY);
        }
        let mut effects = Effects::default();
        effects.push(Effect {
            node,
            generation: 2,
            events: WatchMask::MODIFY,
            terminal: None,
        });
        let mut wakes = WakeBatch::new(table.limit()).unwrap();
        table.publish(&effects, &mut wakes);
        assert_eq!(wakes.remaining(), 24);
        for id in 1..=24 {
            let info = table.query(id, 9).unwrap();
            assert_eq!(info.generation, 2);
            assert!(info.pending.contains(WatchMask::MODIFY));
        }
    }

    #[test]
    fn publish_wakes_each_task_once_and_terminal_is_sticky() {
        let node = NodeId::from_raw(11).unwrap();
        let mut table = Table::new(3).unwrap();
        let first = table.allocate_id().unwrap();
        let second = table.allocate_id().unwrap();
        table.install(first, 1, node, 41, 1, WatchMask::DELETE);
        table.install(second, 1, node, 41, 1, WatchMask::DELETE);
        let mut effects = Effects::default();
        effects.push(Effect {
            node,
            generation: 4,
            events: WatchMask::DELETE,
            terminal: Some(WatchReason::NodeDeleted),
        });
        let mut wakes = WakeBatch::new(table.limit()).unwrap();
        table.publish(&effects, &mut wakes);
        assert_eq!(wakes.remaining(), 1);
        assert_eq!(
            table.query(first, 1).unwrap().reason,
            WatchReason::NodeDeleted
        );
        assert_eq!(
            table.query(second, 1).unwrap().reason,
            WatchReason::NodeDeleted
        );
    }
}
