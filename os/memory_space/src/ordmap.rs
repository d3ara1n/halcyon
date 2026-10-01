//! 有界有序索引：arena 承载的 AVL 树。
//!
//! 供 region ledger（按起始地址）与 backing 分段（按逻辑页 offset）共用。
//! 节点槽在准备阶段经 [`OrdMap::reserve`] 预取；提交/释放路径上的
//! [`OrdMap::insert`]、[`OrdMap::remove`] 只消费或归还已有槽，不分配。
//! 每次操作的工作量受树高约束：AVL 高度 ≤ 1.44·log2(n+2)，无全表搬移或排序。

use alloc::vec::Vec;

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
pub struct ReserveFailed;

pub struct Iter<'a, K, V> {
    map: &'a OrdMap<K, V>,
    next: Option<K>,
}

impl<'a, K: Ord + Copy, V> Iterator for Iter<'a, K, V> {
    type Item = (K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.next?;
        let entry = self.map.floor(&key).filter(|(k, _)| *k == key)?;
        self.next = self.map.after(&key).map(|(k, _)| k);
        Some(entry)
    }
}

pub struct OrdMap<K, V> {
    nodes: Vec<Node<K, V>>,
    free: Vec<u32>,
    /// 已承诺给尚未插入的条目的空闲槽数。
    reserved: usize,
    root: u32,
    len: usize,
}

impl<K: Ord + Copy, V> Default for OrdMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord + Copy, V> OrdMap<K, V> {
    pub const fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
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

    /// 预取 `n` 个节点槽。成功后随后的 `n` 次 insert 不分配。
    ///
    /// 可用槽 = 回收槽（free）+ 已保留但未使用的 arena 容量；free 容量始终
    /// 不小于 arena 容量，故 remove 归还槽也不分配。
    pub fn reserve(&mut self, n: usize) -> Result<(), ReserveFailed> {
        let spare = self.free.len() + (self.nodes.capacity() - self.nodes.len());
        let need = self.reserved + n;
        if need > spare {
            let grow = need - spare;
            self.nodes
                .try_reserve_exact(grow)
                .map_err(|_| ReserveFailed)?;
            let cap = self.nodes.capacity();
            if cap > u32::MAX as usize {
                return Err(ReserveFailed);
            }
            self.free
                .try_reserve_exact(cap - self.free.len())
                .map_err(|_| ReserveFailed)?;
        }
        self.reserved = need;
        Ok(())
    }

    /// 预先扩充存储容量而不产生预取额度；之后 `reserve` 在容量内不分配。
    pub fn grow(&mut self, n: usize) -> Result<(), ReserveFailed> {
        self.nodes.try_reserve_exact(n).map_err(|_| ReserveFailed)?;
        let cap = self.nodes.capacity();
        if cap > u32::MAX as usize {
            return Err(ReserveFailed);
        }
        self.free
            .try_reserve_exact(cap - self.free.len())
            .map_err(|_| ReserveFailed)
    }

    /// 自 `start`（含）起的有序迭代；每步 O(log n)，总工作量随实际遍历条目数。
    pub fn iter_from(&self, start: K) -> Iter<'_, K, V> {
        Iter {
            map: self,
            next: self.ceil(&start).map(|(k, _)| k),
        }
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
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

impl<K: Ord + Copy + core::fmt::Debug, V: core::fmt::Debug> core::fmt::Debug for OrdMap<K, V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec::Vec;

    fn keys(map: &OrdMap<u64, u64>) -> Vec<u64> {
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
        let mut map = OrdMap::<u64, u64>::new();
        let result = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| map.insert(1, 1)));
        assert!(result.is_err());
    }

    #[test]
    fn reserved_inserts_do_not_grow_storage() {
        let mut map = OrdMap::<u64, u64>::new();
        map.reserve(64).unwrap();
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
    fn ordered_queries_match_reference() {
        let mut map = OrdMap::<u64, u64>::new();
        let mut reference = std::collections::BTreeMap::new();
        let mut x: u64 = 12345;
        for _ in 0..2000 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let k = (x >> 33) % 500;
            if x & 1 == 0 && !reference.contains_key(&k) {
                map.reserve(1).unwrap();
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
        let mut map = OrdMap::<u64, ()>::new();
        let n = 4096;
        map.reserve(n).unwrap();
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
