//! 有界有序索引：arena 承载的 AVL 树。
//!
//! 供 region ledger（按起始地址）与 backing 分段（按逻辑页 offset）共用。
//! 节点槽在准备阶段经 [`OrdMap::reserve`] 预取；提交/释放路径上的
//! [`OrdMap::insert`]、[`OrdMap::remove`] 只消费或归还已有槽，不分配。
//! 每次操作的工作量受树高约束：AVL 高度 ≤ 1.44·log2(n+2)，无全表搬移或排序。

use funded_storage::{Buffer, Funding, StorageError};

const NIL: u32 = u32::MAX;

struct Node<K, V> {
    key: K,
    value: Option<V>,
    left: u32,
    right: u32,
    height: u8,
}

/// 预取失败：元数据存储不足。调用方须在改动旧状态前把它报为准备失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveFailed {
    Overflow,
    CapacityChanged,
}

/// 两份实际分配的共同 owner；同时准备、同时发布，旧块锁外释放。
pub struct OrdMapStorage<K, V, C> {
    nodes: Buffer<Node<K, V>, C>,
    free: Buffer<u32, C>,
}
impl<K, V, C> OrdMapStorage<K, V, C> {
    pub const fn new() -> Self {
        Self {
            nodes: Buffer::new(),
            free: Buffer::new(),
        }
    }
    pub fn try_empty<F: Funding<Charge = C>>(
        capacity: usize,
        payer: &F,
    ) -> Result<Self, StorageError<F::Error>> {
        if capacity > u32::MAX as usize {
            return Err(StorageError::Layout);
        }
        let nodes = Buffer::try_empty(capacity, payer)?;
        let free = Buffer::try_empty(capacity, payer)?;
        Ok(Self { nodes, free })
    }
    pub fn capacity(&self) -> usize {
        self.nodes.capacity().min(self.free.capacity())
    }
}
impl<K, V, C> Default for OrdMapStorage<K, V, C> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Iter<'a, K: Copy, V, C> {
    map: &'a OrdMap<K, V, C>,
    next: Option<K>,
}

impl<'a, K: Ord + Copy, V, C> Iterator for Iter<'a, K, V, C> {
    type Item = (K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.next?;
        let entry = self.map.floor(&key).filter(|(k, _)| *k == key)?;
        self.next = self.map.after(&key).map(|(k, _)| k);
        Some(entry)
    }
}

pub struct OrdMap<K: Copy, V, C> {
    nodes: Buffer<Node<K, V>, C>,
    free: Buffer<u32, C>,
    /// 已承诺给尚未插入的条目的空闲槽数。
    reserved: usize,
    root: u32,
    len: usize,
}

impl<K: Copy, V, C> Drop for OrdMap<K, V, C> {
    fn drop(&mut self) {
        if self.len == 0 {
            // remove 已取走每个 V，key 为 Copy、链接为整数；不存在析构义务。
            // 不在最终原子重新遍历节点高水位，付款仍随分配 owner 正常退回。
            self.nodes.forget_values();
        }
    }
}

impl<K: Ord + Copy, V, C> Default for OrdMap<K, V, C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Copy, V, C> OrdMap<K, V, C> {
    pub const fn new() -> Self {
        Self {
            nodes: Buffer::new(),
            free: Buffer::new(),
            reserved: 0,
            root: NIL,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 已预取但尚未消费的槽数。
    pub fn reserved(&self) -> usize {
        self.reserved
    }

    /// 已摘除条目返还逻辑槽；容量与两份付款 owner 仍原样保留。
    pub(crate) fn reserve_recycled(&mut self, n: usize) {
        let common = self.nodes.capacity().min(self.free.capacity());
        let spare = self.free.len() + (common - self.nodes.len());
        assert!(
            n <= self.free.len() && self.reserved <= spare && n <= spare - self.reserved,
            "recycled reservation exceeds guaranteed storage"
        );
        self.reserved += n;
    }

    #[cfg(test)]
    pub(crate) fn storage_capacities(&self) -> (usize, usize) {
        (self.nodes.capacity(), self.free.capacity())
    }

    pub fn capacity(&self) -> usize {
        self.nodes.capacity().min(self.free.capacity())
    }

    fn required(&self, n: usize) -> Result<usize, ReserveFailed> {
        let need = self
            .reserved
            .checked_add(n)
            .ok_or(ReserveFailed::Overflow)?;
        let append = need.saturating_sub(self.free.len());
        self.nodes
            .len()
            .checked_add(append)
            .filter(|v| *v <= u32::MAX as usize)
            .ok_or(ReserveFailed::Overflow)
    }

    pub fn storage_ready(
        &self,
        n: usize,
        prepared: &OrdMapStorage<K, V, C>,
    ) -> Result<bool, ReserveFailed> {
        let required = self.required(n)?;
        Ok(required <= self.capacity() || required <= prepared.capacity())
    }

    /// 只读需求，不接受credit；新准备块在原锁外付费。
    pub fn growth_capacity(&self, n: usize) -> Result<usize, ReserveFailed> {
        let required = self.required(n)?;
        self.capacity()
            .checked_mul(2)
            .and_then(|v| v.checked_add(n))
            .map(|v| v.max(required).min(u32::MAX as usize))
            .ok_or(ReserveFailed::Overflow)
    }

    /// 调用者可先尝试复用；增长重试使用growth_capacity，避免空候选振荡。
    pub fn storage_capacity(&self, n: usize) -> Result<usize, ReserveFailed> {
        if self.required(n)? <= self.capacity() {
            Ok(0)
        } else {
            self.growth_capacity(n)
        }
    }

    /// 同次换两块，不分配、不退费；prepared 交回旧空分配。
    pub fn install_storage(
        &mut self,
        prepared: &mut OrdMapStorage<K, V, C>,
    ) -> Result<(), ReserveFailed> {
        if prepared.capacity() < self.required(0)? {
            return Err(ReserveFailed::CapacityChanged);
        }
        assert!(
            prepared.nodes.is_empty() && prepared.free.is_empty(),
            "prepared index storage contains data"
        );
        // 两边均已完整准备；len/free/预约关系保证第二步不会在第一步之后失败。
        self.nodes
            .replace_storage(&mut prepared.nodes)
            .expect("prepared arena fits live nodes");
        self.free
            .replace_storage(&mut prepared.free)
            .expect("prepared free slots fit logical credits");
        Ok(())
    }

    /// 仅消费现成存储，成功后 n 次 insert/remove 无分配。
    pub fn reserve(
        &mut self,
        n: usize,
        prepared: &mut OrdMapStorage<K, V, C>,
    ) -> Result<(), ReserveFailed> {
        let required = self.required(n)?;
        if required > self.capacity() {
            if prepared.capacity() < required {
                return Err(ReserveFailed::CapacityChanged);
            }
            self.install_storage(prepared)?;
        }
        self.reserved = self
            .reserved
            .checked_add(n)
            .expect("reservation arithmetic already checked");
        Ok(())
    }

    /// 全部业务值和逻辑credit收束后摘出私有容量，owner必须锁外析构。
    pub fn take_empty_storage(&mut self) -> OrdMapStorage<K, V, C> {
        assert!(
            self.is_empty() && self.reserved == 0,
            "index storage retired with live responsibility"
        );
        self.nodes.forget_values();
        let nodes = core::mem::take(&mut self.nodes);
        let free = core::mem::take(&mut self.free);
        self.root = NIL;
        OrdMapStorage { nodes, free }
    }

    /// 自 `start`（含）起的有序迭代；每步 O(log n)，总工作量随实际遍历条目数。
    pub fn iter_from(&self, start: K) -> Iter<'_, K, V, C> {
        Iter {
            map: self,
            next: self.ceil(&start).map(|(k, _)| k),
        }
    }

    pub fn iter(&self) -> Iter<'_, K, V, C> {
        Iter {
            map: self,
            next: self.first().map(|(k, _)| k),
        }
    }

    /// 撤销未消费的预取（准备失败或回滚）。槽留在 free 中供复用。
    pub fn unreserve(&mut self, n: usize) {
        assert!(
            n <= self.reserved,
            "unreserve exceeds outstanding reservation"
        );
        self.reserved -= n;
    }

    /// 插入新键，消费一个预取槽。键已存在为结构性错误。
    pub fn insert(&mut self, key: K, value: V) {
        assert!(self.reserved > 0, "OrdMap insert without reserved slot");
        self.reserved -= 1;
        let fresh = Node {
            key,
            value: Some(value),
            left: NIL,
            right: NIL,
            height: 1,
        };
        let slot = match self.free.pop() {
            Some(slot) => {
                self.nodes[slot as usize] = fresh;
                slot
            }
            None => {
                debug_assert!(self.nodes.len() < self.nodes.capacity());
                self.nodes.push(fresh);
                (self.nodes.len() - 1) as u32
            }
        };
        self.root = self.insert_at(self.root, slot);
        self.len += 1;
    }

    /// 删除并返回键对应的值；槽归还 free（不释放存储）。
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let mut removed = NIL;
        self.root = self.remove_at(self.root, key, &mut removed);
        if removed == NIL {
            return None;
        }
        self.len -= 1;
        let value = self.nodes[removed as usize].value.take();
        debug_assert!(self.free.len() < self.free.capacity());
        self.free.push(removed);
        value
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let slot = self.find(key);
        (slot != NIL).then(|| self.nodes[slot as usize].value.as_ref().unwrap())
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let slot = self.find(key);
        if slot == NIL {
            return None;
        }
        self.nodes[slot as usize].value.as_mut()
    }

    /// 最大的 ≤ key 的条目（区间查找：包含某点的段/region 起点）。
    pub fn floor(&self, key: &K) -> Option<(K, &V)> {
        let mut cur = self.root;
        let mut best = NIL;
        while cur != NIL {
            let node = &self.nodes[cur as usize];
            if node.key <= *key {
                best = cur;
                cur = node.right;
            } else {
                cur = node.left;
            }
        }
        self.entry(best)
    }

    /// 最小的 ≥ key 的条目（游标续跑）。
    pub fn ceil(&self, key: &K) -> Option<(K, &V)> {
        let mut cur = self.root;
        let mut best = NIL;
        while cur != NIL {
            let node = &self.nodes[cur as usize];
            if node.key >= *key {
                best = cur;
                cur = node.left;
            } else {
                cur = node.right;
            }
        }
        self.entry(best)
    }

    /// 最小的 > key 的条目。
    pub fn after(&self, key: &K) -> Option<(K, &V)> {
        let mut cur = self.root;
        let mut best = NIL;
        while cur != NIL {
            let node = &self.nodes[cur as usize];
            if node.key > *key {
                best = cur;
                cur = node.left;
            } else {
                cur = node.right;
            }
        }
        self.entry(best)
    }

    pub fn first(&self) -> Option<(K, &V)> {
        let mut cur = self.root;
        if cur == NIL {
            return None;
        }
        while self.nodes[cur as usize].left != NIL {
            cur = self.nodes[cur as usize].left;
        }
        self.entry(cur)
    }

    /// 当前树高；单测用以核对结构上界。
    pub fn height(&self) -> usize {
        self.h(self.root) as usize
    }

    fn entry(&self, slot: u32) -> Option<(K, &V)> {
        (slot != NIL).then(|| {
            let node = &self.nodes[slot as usize];
            (node.key, node.value.as_ref().unwrap())
        })
    }

    fn find(&self, key: &K) -> u32 {
        let mut cur = self.root;
        while cur != NIL {
            let node = &self.nodes[cur as usize];
            match key.cmp(&node.key) {
                core::cmp::Ordering::Less => cur = node.left,
                core::cmp::Ordering::Greater => cur = node.right,
                core::cmp::Ordering::Equal => return cur,
            }
        }
        NIL
    }

    fn h(&self, slot: u32) -> u8 {
        if slot == NIL {
            0
        } else {
            self.nodes[slot as usize].height
        }
    }

    fn update(&mut self, slot: u32) {
        let node = &self.nodes[slot as usize];
        let height = 1 + self.h(node.left).max(self.h(node.right));
        self.nodes[slot as usize].height = height;
    }

    fn rotate_right(&mut self, slot: u32) -> u32 {
        let left = self.nodes[slot as usize].left;
        self.nodes[slot as usize].left = self.nodes[left as usize].right;
        self.nodes[left as usize].right = slot;
        self.update(slot);
        self.update(left);
        left
    }

    fn rotate_left(&mut self, slot: u32) -> u32 {
        let right = self.nodes[slot as usize].right;
        self.nodes[slot as usize].right = self.nodes[right as usize].left;
        self.nodes[right as usize].left = slot;
        self.update(slot);
        self.update(right);
        right
    }

    fn balance(&mut self, slot: u32) -> u32 {
        self.update(slot);
        let left = self.nodes[slot as usize].left;
        let right = self.nodes[slot as usize].right;
        let factor = self.h(left) as i16 - self.h(right) as i16;
        if factor > 1 {
            let l = self.nodes[left as usize].left;
            let r = self.nodes[left as usize].right;
            if self.h(l) < self.h(r) {
                self.nodes[slot as usize].left = self.rotate_left(left);
            }
            return self.rotate_right(slot);
        }
        if factor < -1 {
            let l = self.nodes[right as usize].left;
            let r = self.nodes[right as usize].right;
            if self.h(r) < self.h(l) {
                self.nodes[slot as usize].right = self.rotate_right(right);
            }
            return self.rotate_left(slot);
        }
        slot
    }

    fn insert_at(&mut self, cur: u32, slot: u32) -> u32 {
        if cur == NIL {
            return slot;
        }
        let key = self.nodes[slot as usize].key;
        match key.cmp(&self.nodes[cur as usize].key) {
            core::cmp::Ordering::Less => {
                let child = self.insert_at(self.nodes[cur as usize].left, slot);
                self.nodes[cur as usize].left = child;
            }
            core::cmp::Ordering::Greater => {
                let child = self.insert_at(self.nodes[cur as usize].right, slot);
                self.nodes[cur as usize].right = child;
            }
            core::cmp::Ordering::Equal => panic!("OrdMap duplicate key"),
        }
        self.balance(cur)
    }

    /// 摘出子树最小节点，返回 (新子树根, 最小节点)。
    fn take_min(&mut self, cur: u32) -> (u32, u32) {
        let left = self.nodes[cur as usize].left;
        if left == NIL {
            return (self.nodes[cur as usize].right, cur);
        }
        let (child, min) = self.take_min(left);
        self.nodes[cur as usize].left = child;
        (self.balance(cur), min)
    }

    fn remove_at(&mut self, cur: u32, key: &K, removed: &mut u32) -> u32 {
        if cur == NIL {
            return NIL;
        }
        match key.cmp(&self.nodes[cur as usize].key) {
            core::cmp::Ordering::Less => {
                let child = self.remove_at(self.nodes[cur as usize].left, key, removed);
                self.nodes[cur as usize].left = child;
            }
            core::cmp::Ordering::Greater => {
                let child = self.remove_at(self.nodes[cur as usize].right, key, removed);
                self.nodes[cur as usize].right = child;
            }
            core::cmp::Ordering::Equal => {
                *removed = cur;
                let left = self.nodes[cur as usize].left;
                let right = self.nodes[cur as usize].right;
                if right == NIL {
                    return left;
                }
                let (rest, min) = self.take_min(right);
                self.nodes[min as usize].left = left;
                self.nodes[min as usize].right = rest;
                return self.balance(min);
            }
        }
        self.balance(cur)
    }
}

impl<K: Ord + Copy + core::fmt::Debug, V: core::fmt::Debug, C> core::fmt::Debug
    for OrdMap<K, V, C>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec::Vec;
    struct TestFunding;
    impl Funding for TestFunding {
        type Charge = ();
        type Error = ();
        fn charge(&self, _: core::alloc::Layout) -> Result<(), ()> {
            Ok(())
        }
    }
    fn reserve<K: Ord + Copy, V>(
        map: &mut OrdMap<K, V, ()>,
        n: usize,
    ) -> Result<(), ReserveFailed> {
        let mut storage = OrdMapStorage::try_empty(map.storage_capacity(n)?, &TestFunding).unwrap();
        map.reserve(n, &mut storage)
    }

    struct Charge(std::rc::Rc<core::cell::Cell<usize>>);
    impl Drop for Charge {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }
    struct Meter {
        live: std::rc::Rc<core::cell::Cell<usize>>,
        calls: core::cell::Cell<usize>,
        deny: Option<usize>,
    }
    impl Funding for Meter {
        type Charge = Charge;
        type Error = ();
        fn charge(&self, _: core::alloc::Layout) -> Result<Charge, ()> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            if self.deny == Some(call) {
                return Err(());
            }
            self.live.set(self.live.get() + 1);
            Ok(Charge(self.live.clone()))
        }
    }

    #[test]
    fn second_buffer_funding_failure_returns_first_charge_without_publication() {
        let payer = Meter {
            live: Default::default(),
            calls: Default::default(),
            deny: Some(2),
        };
        let map = OrdMap::<u64, (), Charge>::new();
        assert!(matches!(
            OrdMapStorage::<u64, (), Charge>::try_empty(8, &payer),
            Err(StorageError::Funding(()))
        ));
        assert_eq!(payer.calls.get(), 2);
        assert_eq!(payer.live.get(), 0);
        assert_eq!((map.capacity(), map.reserved(), map.len()), (0, 0, 0));
    }

    #[test]
    fn growth_retry_stays_nonempty_after_competing_credit_is_returned() {
        let mut map = OrdMap::<u64, (), ()>::new();
        reserve(&mut map, 4).unwrap();
        let mut empty = OrdMapStorage::new();
        assert_eq!(
            map.reserve(1, &mut empty),
            Err(ReserveFailed::CapacityChanged)
        );
        map.unreserve(4);
        assert_eq!(map.storage_capacity(1).unwrap(), 0);
        // 重试已经发生，后来空出槽也不能再次采样空候选，否则可无限振荡。
        let capacity = map.growth_capacity(1).unwrap();
        assert!(capacity > map.capacity() * 2);
        let mut retry = OrdMapStorage::try_empty(capacity, &TestFunding).unwrap();
        map.reserve(1, &mut retry).unwrap();
        map.unreserve(1);
    }

    #[test]
    fn logical_removal_and_storage_exchange_keep_charges_with_actual_owners() {
        let payer = Meter {
            live: Default::default(),
            calls: Default::default(),
            deny: None,
        };
        let mut map = OrdMap::<u64, (), Charge>::new();
        let mut storage = OrdMapStorage::try_empty(4, &payer).unwrap();
        map.reserve(4, &mut storage).unwrap();
        for key in 0..4 {
            map.insert(key, ());
        }
        let mut larger = OrdMapStorage::try_empty(12, &payer).unwrap();
        map.reserve(8, &mut larger).unwrap();
        assert_eq!(payer.live.get(), 4);
        drop(larger); // 原两块仅在外层释放时退款。
        assert_eq!(payer.live.get(), 2);
        for key in 0..4 {
            map.remove(&key);
        }
        map.unreserve(8);
        assert_eq!(payer.live.get(), 2); // 空逻辑索引仍保有实际容量。
        let retired = map.take_empty_storage();
        drop(map);
        assert_eq!(payer.live.get(), 2);
        drop(retired);
        assert_eq!(payer.live.get(), 0);
    }

    fn keys(map: &OrdMap<u64, u64, ()>) -> Vec<u64> {
        let mut out = Vec::new();
        let mut cur = map.first().map(|(k, _)| k);
        while let Some(k) = cur {
            out.push(k);
            cur = map.after(&k).map(|(k, _)| k);
        }
        out
    }

    #[test]
    fn insert_without_reserve_is_rejected() {
        let mut map = OrdMap::<u64, u64, ()>::new();
        let result = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| map.insert(1, 1)));
        assert!(result.is_err());
    }

    #[test]
    fn reserved_inserts_do_not_grow_storage() {
        let mut map = OrdMap::<u64, u64, ()>::new();
        reserve(&mut map, 64).unwrap();
        let cap = (map.nodes.capacity(), map.free.capacity());
        for k in (0..64).rev() {
            map.insert(k * 3, k);
        }
        assert_eq!((map.nodes.capacity(), map.free.capacity()), cap);
        assert_eq!(map.reserved(), 0);
        for k in 0..32 {
            assert_eq!(map.remove(&(k * 3)), Some(k));
        }
        assert_eq!((map.nodes.capacity(), map.free.capacity()), cap);
    }

    #[test]
    fn consecutive_reservations_cover_all_outstanding_inserts() {
        let mut map = OrdMap::<u64, (), ()>::new();
        reserve(&mut map, 8).unwrap();
        reserve(&mut map, 8).unwrap();
        let capacity = (map.nodes.capacity(), map.free.capacity());
        for key in 0..16 {
            map.insert(key, ());
        }
        assert_eq!((map.nodes.capacity(), map.free.capacity()), capacity);
        for key in 0..8 {
            map.remove(&key);
        }
        reserve(&mut map, 8).unwrap();
        reserve(&mut map, 8).unwrap();
        let capacity = (map.nodes.capacity(), map.free.capacity());
        for key in 16..32 {
            map.insert(key, ());
        }
        assert_eq!((map.nodes.capacity(), map.free.capacity()), capacity);
        assert_eq!(map.len(), 24);
    }

    #[test]
    fn reservation_failure_does_not_publish_credit_or_skip_free_storage() {
        let mut map = OrdMap::<u64, (), ()>::new();
        reserve(&mut map, 2).unwrap();
        assert!(reserve(&mut map, usize::MAX).is_err());
        assert_eq!(map.reserved(), 2);
        assert_eq!(map.nodes.capacity(), map.free.capacity());
        let mut stale = OrdMapStorage::new();
        assert_eq!(
            map.reserve(8, &mut stale),
            Err(ReserveFailed::CapacityChanged)
        );
        assert_eq!(map.reserved(), 2);
        for key in 0..2 {
            map.insert(key, ());
        }
        let capacity = map.free.capacity();
        for key in 0..2 {
            map.remove(&key);
        }
        assert_eq!(map.free.capacity(), capacity);
    }

    #[test]
    fn ordered_queries_match_reference() {
        let mut map = OrdMap::<u64, u64, ()>::new();
        let mut reference = std::collections::BTreeMap::new();
        let mut x: u64 = 12345;
        for _ in 0..2000 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let k = (x >> 33) % 500;
            if x & 1 == 0 && !reference.contains_key(&k) {
                reserve(&mut map, 1).unwrap();
                map.insert(k, k + 7);
                reference.insert(k, k + 7);
            } else {
                assert_eq!(map.remove(&k), reference.remove(&k));
            }
            let probe = (x >> 13) % 520;
            assert_eq!(
                map.floor(&probe).map(|(k, _)| k),
                reference.range(..=probe).next_back().map(|(k, _)| *k)
            );
            assert_eq!(
                map.ceil(&probe).map(|(k, _)| k),
                reference.range(probe..).next().map(|(k, _)| *k)
            );
        }
        assert_eq!(keys(&map), reference.keys().copied().collect::<Vec<_>>());
    }

    #[test]
    fn height_stays_logarithmic_under_sequential_insert() {
        let mut map = OrdMap::<u64, (), ()>::new();
        let n = 4096;
        reserve(&mut map, n).unwrap();
        for k in 0..n as u64 {
            map.insert(k, ());
        }
        // AVL 上界 1.44·log2(n+2)。
        let bound = (1.45 * ((n + 2) as f64).log2()).ceil() as usize;
        assert!(
            map.height() <= bound,
            "height {} > bound {}",
            map.height(),
            bound
        );
    }
}
