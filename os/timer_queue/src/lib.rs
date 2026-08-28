#![no_std]

//! 可取消期限的索引最小堆。
//!
//! 每一项由稳定 token 标识。token 包含拥有者 hart slot、arena 槽位与
//! generation；槽位复用时 generation 前进，旧 token 幂等失效。

extern crate alloc;

use alloc::vec::Vec;

const OWNER_BITS: u32 = 8;
const SLOT_BITS: u32 = 28;
const GENERATION_BITS: u32 = 28;
const OWNER_MASK: u64 = (1 << OWNER_BITS) - 1;
const SLOT_MASK: u64 = (1 << SLOT_BITS) - 1;
const GENERATION_MASK: u64 = (1 << GENERATION_BITS) - 1;
/// 保留全一值给 WaitContext 的 Closed 状态。
const GENERATION_MAX: u32 = GENERATION_MASK as u32 - 1;

/// 稳定的期限注册标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerToken(u64);

impl TimerToken {
    /// token 的原始稳定表示；供原子 registration 状态保存。
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 从先前由 [`Self::raw`] 取得的值恢复 token。
    ///
    /// 调用方只能传入非零、非全一的既有 token。
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn owner_slot(self) -> usize {
        (self.0 & OWNER_MASK) as usize
    }

    fn slot(self) -> usize {
        (((self.0 >> OWNER_BITS) & SLOT_MASK) as usize) - 1
    }

    fn generation(self) -> u32 {
        (self.0 >> (OWNER_BITS + SLOT_BITS)) as u32
    }

    fn new(owner_slot: usize, slot: usize, generation: u32) -> Self {
        debug_assert!(owner_slot <= OWNER_MASK as usize);
        debug_assert!(slot < SLOT_MASK as usize);
        debug_assert!(generation != 0 && generation <= GENERATION_MAX);
        Self(
            ((generation as u64) << (OWNER_BITS + SLOT_BITS))
                | (((slot as u64) + 1) << OWNER_BITS)
                | owner_slot as u64,
        )
    }
}

/// 注册失败：增长预留未能完成，队列语义状态保持不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationFailed;

enum SlotState<T> {
    Vacant {
        next: Option<usize>,
    },
    Occupied {
        expires_at: u64,
        value: T,
        heap_index: usize,
    },
    Retired,
}

struct Slot<T> {
    generation: u32,
    state: SlotState<T>,
}

/// 纯逻辑、可取消的期限队列。
///
/// 注册、注销与到期弹出都是 O(log n)，堆顶读取为 O(1)。注册在任何
/// 语义状态变更前预留所需 arena 与 heap 容量，因此 OOM 不留 live entry。
pub struct TimerQueue<T> {
    owner_slot: Option<usize>,
    arena: Vec<Slot<T>>,
    free_head: Option<usize>,
    heap: Vec<usize>,
}

impl<T> TimerQueue<T> {
    /// 建立已绑定到一个 hart slot 的队列。
    pub const fn new(owner_slot: usize) -> Self {
        assert!(owner_slot <= OWNER_MASK as usize);
        Self {
            owner_slot: Some(owner_slot),
            arena: Vec::new(),
            free_head: None,
            heap: Vec::new(),
        }
    }

    /// 建立尚待绑定的静态队列。只允许其所属 hart 首次使用时绑定。
    pub const fn unbound() -> Self {
        Self {
            owner_slot: None,
            arena: Vec::new(),
            free_head: None,
            heap: Vec::new(),
        }
    }

    /// 绑定静态队列的所属 hart。重复绑定只接受同一 owner。
    pub fn bind_owner(&mut self, owner_slot: usize) -> bool {
        if owner_slot > OWNER_MASK as usize {
            return false;
        }
        match self.owner_slot {
            Some(owner) => owner == owner_slot,
            None => {
                self.owner_slot = Some(owner_slot);
                true
            }
        }
    }

    /// 注册一项相对调用者不透明的值。
    pub fn try_register(
        &mut self,
        expires_at: u64,
        value: T,
    ) -> Result<TimerToken, AllocationFailed> {
        let Some(owner_slot) = self.owner_slot else {
            return Err(AllocationFailed);
        };
        // 所有可能增长均在变更 free 链、arena 和 heap 前完成。
        self.heap.try_reserve(1).map_err(|_| AllocationFailed)?;
        if self.free_head.is_none() {
            self.arena.try_reserve(1).map_err(|_| AllocationFailed)?;
            if self.arena.len() >= SLOT_MASK as usize {
                return Err(AllocationFailed);
            }
        }

        let (slot, generation) = match self.free_head {
            Some(index) => {
                let entry = &mut self.arena[index];
                let SlotState::Vacant { next } = entry.state else {
                    unreachable!("timer free list names a non-vacant slot")
                };
                self.free_head = next;
                (index, entry.generation)
            }
            None => {
                let index = self.arena.len();
                self.arena.push(Slot {
                    generation: 1,
                    state: SlotState::Retired,
                });
                (index, 1)
            }
        };
        let heap_index = self.heap.len();
        self.arena[slot].state = SlotState::Occupied {
            expires_at,
            value,
            heap_index,
        };
        self.heap.push(slot);
        self.sift_up(heap_index);
        Ok(TimerToken::new(owner_slot, slot, generation))
    }

    /// 幂等注销：旧 generation、已弹出或错误 owner queue 的 token 返回 None。
    pub fn cancel(&mut self, token: TimerToken) -> Option<T> {
        if self.owner_slot != Some(token.owner_slot()) {
            return None;
        }
        let slot = self.valid_slot(token)?;
        let heap_index = match self.arena[slot].state {
            SlotState::Occupied { heap_index, .. } => heap_index,
            SlotState::Vacant { .. } | SlotState::Retired => return None,
        };
        Some(self.remove_heap_index(heap_index))
    }

    /// 读取最早到期点；不移除。
    pub fn peek_expires_at(&self) -> Option<u64> {
        self.heap.first().map(|slot| self.expires_at(*slot))
    }

    /// 弹出一项到期值。token 的 owner 必须与本队列的结构性绑定一致。
    pub fn pop_expired(&mut self, now: u64) -> Option<(TimerToken, T)> {
        let owner_slot = self.owner_slot?;
        let slot = *self.heap.first()?;
        if self.expires_at(slot) > now {
            return None;
        }
        let generation = self.arena[slot].generation;
        let value = self.remove_heap_index(0);
        Some((TimerToken::new(owner_slot, slot, generation), value))
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    fn valid_slot(&self, token: TimerToken) -> Option<usize> {
        let slot = token.slot();
        let entry = self.arena.get(slot)?;
        (token.generation() != 0 && entry.generation == token.generation()).then_some(slot)
    }

    fn expires_at(&self, slot: usize) -> u64 {
        match self.arena[slot].state {
            SlotState::Occupied { expires_at, .. } => expires_at,
            SlotState::Vacant { .. } | SlotState::Retired => {
                unreachable!("heap names a non-live timer slot")
            }
        }
    }

    fn set_heap_index(&mut self, slot: usize, heap_index: usize) {
        match &mut self.arena[slot].state {
            SlotState::Occupied {
                heap_index: current,
                ..
            } => *current = heap_index,
            SlotState::Vacant { .. } | SlotState::Retired => {
                unreachable!("heap names a non-live timer slot")
            }
        }
    }

    fn remove_heap_index(&mut self, heap_index: usize) -> T {
        let slot = self.heap.swap_remove(heap_index);
        if heap_index < self.heap.len() {
            let replacement = self.heap[heap_index];
            self.set_heap_index(replacement, heap_index);
            if heap_index > 0
                && self.expires_at(replacement) < self.expires_at(self.heap[(heap_index - 1) / 2])
            {
                self.sift_up(heap_index);
            } else {
                self.sift_down(heap_index);
            }
        }
        let state = core::mem::replace(&mut self.arena[slot].state, SlotState::Retired);
        let SlotState::Occupied { value, .. } = state else {
            unreachable!("removed timer slot was not live")
        };
        self.recycle(slot);
        value
    }

    fn recycle(&mut self, slot: usize) {
        let entry = &mut self.arena[slot];
        if entry.generation == GENERATION_MAX {
            entry.state = SlotState::Retired;
            return;
        }
        entry.generation += 1;
        entry.state = SlotState::Vacant {
            next: self.free_head,
        };
        self.free_head = Some(slot);
    }

    fn sift_up(&mut self, mut child: usize) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if self.expires_at(self.heap[parent]) <= self.expires_at(self.heap[child]) {
                break;
            }
            self.heap.swap(parent, child);
            self.set_heap_index(self.heap[parent], parent);
            self.set_heap_index(self.heap[child], child);
            child = parent;
        }
    }

    fn sift_down(&mut self, mut parent: usize) {
        loop {
            let left = parent * 2 + 1;
            if left >= self.heap.len() {
                return;
            }
            let right = left + 1;
            let child = if right < self.heap.len()
                && self.expires_at(self.heap[right]) < self.expires_at(self.heap[left])
            {
                right
            } else {
                left
            };
            if self.expires_at(self.heap[parent]) <= self.expires_at(self.heap[child]) {
                return;
            }
            self.heap.swap(parent, child);
            self.set_heap_index(self.heap[parent], parent);
            self.set_heap_index(self.heap[child], child);
            parent = child;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn unordered_registration_pops_in_expiry_order() {
        let mut queue = TimerQueue::new(2);
        queue.try_register(30, 'c').unwrap();
        queue.try_register(10, 'a').unwrap();
        queue.try_register(20, 'b').unwrap();
        assert_eq!(queue.peek_expires_at(), Some(10));
        assert_eq!(queue.pop_expired(9), None);
        assert_eq!(queue.pop_expired(10).map(|(_, value)| value), Some('a'));
        assert_eq!(queue.pop_expired(20).map(|(_, value)| value), Some('b'));
        assert_eq!(queue.pop_expired(30).map(|(_, value)| value), Some('c'));
        assert!(queue.is_empty());
    }

    #[test]
    fn equal_expiry_and_cancel_at_every_heap_position() {
        let mut queue = TimerQueue::new(1);
        let first = queue.try_register(10, 1).unwrap();
        let middle = queue.try_register(10, 2).unwrap();
        let last = queue.try_register(10, 3).unwrap();
        assert_eq!(queue.cancel(first), Some(1));
        assert_eq!(queue.cancel(last), Some(3));
        assert_eq!(queue.cancel(middle), Some(2));
        assert!(queue.is_empty());
    }

    #[test]
    fn reused_slot_invalidates_old_generation() {
        let mut queue = TimerQueue::new(3);
        let old = queue.try_register(1, 7).unwrap();
        assert_eq!(queue.cancel(old), Some(7));
        let new = queue.try_register(2, 8).unwrap();
        assert_ne!(old, new);
        assert_eq!(queue.cancel(old), None);
        assert_eq!(queue.cancel(new), Some(8));
    }

    #[test]
    fn cancellation_is_idempotent_and_does_not_touch_other_entries() {
        let mut queue = TimerQueue::new(4);
        let cancelled = queue.try_register(1, 1).unwrap();
        queue.try_register(2, 2).unwrap();
        assert_eq!(queue.cancel(cancelled), Some(1));
        assert_eq!(queue.cancel(cancelled), None);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop_expired(2).map(|(_, value)| value), Some(2));
    }

    #[test]
    fn owner_binding_rejects_wrong_queue_and_cross_queue_token() {
        let mut owner_one = TimerQueue::new(1);
        let mut owner_two = TimerQueue::<char>::new(2);
        let token = owner_one.try_register(10, 'a').unwrap();
        assert_eq!(owner_two.cancel(token), None);
        assert_eq!(owner_one.len(), 1);
        assert_eq!(owner_one.cancel(token), Some('a'));

        let mut unbound = TimerQueue::<()>::unbound();
        assert!(unbound.bind_owner(3));
        assert!(unbound.bind_owner(3));
        assert!(!unbound.bind_owner(4));
    }

    #[test]
    fn middle_removal_restores_heap_after_sifting_in_both_directions() {
        let mut upward = TimerQueue::new(5);
        let _ = upward.try_register(1, 1).unwrap();
        let _ = upward.try_register(50, 50).unwrap();
        let _ = upward.try_register(2, 2).unwrap();
        let middle = upward.try_register(60, 60).unwrap();
        let _ = upward.try_register(70, 70).unwrap();
        let _ = upward.try_register(3, 3).unwrap();
        let _ = upward.try_register(4, 4).unwrap();
        assert_eq!(upward.cancel(middle), Some(60));
        assert_eq!(
            (0..6)
                .filter_map(|_| upward.pop_expired(u64::MAX).map(|(_, value)| value))
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 50, 70]
        );

        let mut downward = TimerQueue::new(6);
        let _ = downward.try_register(1, 1).unwrap();
        let middle = downward.try_register(50, 50).unwrap();
        let _ = downward.try_register(2, 2).unwrap();
        let _ = downward.try_register(60, 60).unwrap();
        let _ = downward.try_register(70, 70).unwrap();
        let _ = downward.try_register(3, 3).unwrap();
        let _ = downward.try_register(100, 100).unwrap();
        assert_eq!(downward.cancel(middle), Some(50));
        assert_eq!(
            (0..6)
                .filter_map(|_| downward.pop_expired(u64::MAX).map(|(_, value)| value))
                .collect::<Vec<_>>(),
            [1, 2, 3, 60, 70, 100]
        );
    }
}
