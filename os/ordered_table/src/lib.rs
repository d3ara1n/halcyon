#![no_std]
#![feature(allocator_api)]

//! 带容量上限和可失败节点准备的 AVL 有序表。
//!
//! 节点只在准备/插入期分配；查找、修改和删除不分配。每个实例携带明确条目
//! 上限，因此树高和单次操作的比较次数都有硬上界。

extern crate alloc;

use alloc::boxed::Box;
use core::cmp::Ordering;

type Link<V> = Option<Box<Node<V>>>;

struct Node<V> {
    key: u64,
    value: V,
    height: u8,
    left: Link<V>,
    right: Link<V>,
}

pub struct PreparedEntry<V>(Box<Node<V>>);

impl<V> PreparedEntry<V> {
    pub fn key(&self) -> u64 {
        self.0.key
    }

    pub fn into_value(self) -> V {
        let Node { value, .. } = *self.0;
        value
    }
}

pub enum InsertError<V> {
    Limit(V),
    Allocation(V),
}

pub struct OrderedTable<V> {
    root: Link<V>,
    len: usize,
    limit: usize,
}

impl<V> OrderedTable<V> {
    pub const fn new(limit: usize) -> Self {
        assert!(limit > 0);
        Self {
            root: None,
            len: 0,
            limit,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn prepare_insert(&self, key: u64, value: V) -> Result<PreparedEntry<V>, InsertError<V>> {
        if self.len == self.limit {
            return Err(InsertError::Limit(value));
        }
        if self.contains_key(key) {
            unreachable!("ordered table key prepared twice");
        }
        allocate_entry(key, value)
    }

    /// 为 Commit 时才确定“复用现项或插入新项”的事务准备候选节点。
    /// key 当前存在时即使表已满也允许准备：若该项在 Commit 前被移除，
    /// 空出的同一容量正好供候选插入；若仍存在，调用方丢弃候选即可。
    pub fn prepare_insert_candidate(
        &self,
        key: u64,
        value: V,
    ) -> Result<PreparedEntry<V>, InsertError<V>> {
        if !self.contains_key(key) && self.len == self.limit {
            return Err(InsertError::Limit(value));
        }
        allocate_entry(key, value)
    }

    pub fn insert_prepared(&mut self, entry: PreparedEntry<V>) {
        assert!(
            self.len < self.limit,
            "ordered table capacity changed after preparation"
        );
        assert!(
            !self.contains_key(entry.0.key),
            "ordered table key changed after preparation"
        );
        self.root = Some(insert_node(self.root.take(), entry.0));
        self.len += 1;
    }

    pub fn try_insert(&mut self, key: u64, value: V) -> Result<(), InsertError<V>> {
        let entry = self.prepare_insert(key, value)?;
        self.insert_prepared(entry);
        Ok(())
    }

    pub fn contains_key(&self, key: u64) -> bool {
        self.get(key).is_some()
    }

    pub fn get(&self, key: u64) -> Option<&V> {
        let mut current = self.root.as_deref();
        while let Some(node) = current {
            match key.cmp(&node.key) {
                Ordering::Less => current = node.left.as_deref(),
                Ordering::Greater => current = node.right.as_deref(),
                Ordering::Equal => return Some(&node.value),
            }
        }
        None
    }

    pub fn get_mut(&mut self, key: u64) -> Option<&mut V> {
        let mut current = self.root.as_deref_mut();
        while let Some(node) = current {
            match key.cmp(&node.key) {
                Ordering::Less => current = node.left.as_deref_mut(),
                Ordering::Greater => current = node.right.as_deref_mut(),
                Ordering::Equal => return Some(&mut node.value),
            }
        }
        None
    }

    pub fn remove(&mut self, key: u64) -> Option<V> {
        let (root, removed) = remove_node(self.root.take(), key);
        self.root = root;
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    pub fn count_matching(&self, predicate: impl Fn(&V) -> bool) -> usize {
        fn count<V>(node: &Link<V>, predicate: &impl Fn(&V) -> bool) -> usize {
            let Some(node) = node else { return 0 };
            count(&node.left, predicate)
                + usize::from(predicate(&node.value))
                + count(&node.right, predicate)
        }
        count(&self.root, &predicate)
    }

    pub fn scan_visible(
        &self,
        visible: impl Fn(&V) -> bool,
        cursor: u64,
        out: &mut [u64],
    ) -> (usize, bool) {
        fn scan<V>(
            node: &Link<V>,
            visible: &impl Fn(&V) -> bool,
            cursor: u64,
            out: &mut [u64],
            actual: &mut usize,
        ) -> bool {
            let Some(node) = node else { return false };
            if node.key <= cursor {
                return scan(&node.right, visible, cursor, out, actual);
            }
            if scan(&node.left, visible, cursor, out, actual) {
                return true;
            }
            if *actual == out.len() || !visible(&node.value) {
                return true;
            }
            out[*actual] = node.key;
            *actual += 1;
            scan(&node.right, visible, cursor, out, actual)
        }

        let mut actual = 0;
        let more = scan(&self.root, &visible, cursor, out, &mut actual);
        (actual, more)
    }
}

fn allocate_entry<V>(key: u64, value: V) -> Result<PreparedEntry<V>, InsertError<V>> {
    let mut allocation = match Box::<Node<V>>::try_new_uninit() {
        Ok(allocation) => allocation,
        Err(_) => return Err(InsertError::Allocation(value)),
    };
    allocation.write(Node {
        key,
        value,
        height: 1,
        left: None,
        right: None,
    });
    // allocation 已在上方完整初始化，且没有引用逃逸。
    Ok(PreparedEntry(unsafe { allocation.assume_init() }))
}

fn height<V>(node: &Link<V>) -> u8 {
    node.as_ref().map_or(0, |node| node.height)
}

fn update_height<V>(node: &mut Node<V>) {
    node.height = 1 + height(&node.left).max(height(&node.right));
}

fn rotate_left<V>(mut root: Box<Node<V>>) -> Box<Node<V>> {
    let mut pivot = root.right.take().expect("AVL left rotation lost pivot");
    root.right = pivot.left.take();
    update_height(&mut root);
    pivot.left = Some(root);
    update_height(&mut pivot);
    pivot
}

fn rotate_right<V>(mut root: Box<Node<V>>) -> Box<Node<V>> {
    let mut pivot = root.left.take().expect("AVL right rotation lost pivot");
    root.left = pivot.right.take();
    update_height(&mut root);
    pivot.right = Some(root);
    update_height(&mut pivot);
    pivot
}

fn rebalance<V>(mut node: Box<Node<V>>) -> Box<Node<V>> {
    update_height(&mut node);
    let balance = i16::from(height(&node.left)) - i16::from(height(&node.right));
    if balance > 1 {
        let left = node.left.as_ref().expect("AVL balance lost left child");
        if height(&left.left) < height(&left.right) {
            node.left = node.left.take().map(rotate_left);
        }
        return rotate_right(node);
    }
    if balance < -1 {
        let right = node.right.as_ref().expect("AVL balance lost right child");
        if height(&right.right) < height(&right.left) {
            node.right = node.right.take().map(rotate_right);
        }
        return rotate_left(node);
    }
    node
}

fn insert_node<V>(root: Link<V>, node: Box<Node<V>>) -> Box<Node<V>> {
    let Some(mut root) = root else { return node };
    match node.key.cmp(&root.key) {
        Ordering::Less => root.left = Some(insert_node(root.left.take(), node)),
        Ordering::Greater => root.right = Some(insert_node(root.right.take(), node)),
        Ordering::Equal => unreachable!("ordered table key inserted twice"),
    }
    rebalance(root)
}

fn take_min<V>(mut node: Box<Node<V>>) -> (Link<V>, Box<Node<V>>) {
    let Some(left) = node.left.take() else {
        return (node.right.take(), node);
    };
    let (new_left, minimum) = take_min(left);
    node.left = new_left;
    (Some(rebalance(node)), minimum)
}

fn remove_node<V>(root: Link<V>, key: u64) -> (Link<V>, Option<V>) {
    let Some(mut root) = root else {
        return (None, None);
    };
    match key.cmp(&root.key) {
        Ordering::Less => {
            let (left, removed) = remove_node(root.left.take(), key);
            root.left = left;
            (Some(rebalance(root)), removed)
        }
        Ordering::Greater => {
            let (right, removed) = remove_node(root.right.take(), key);
            root.right = right;
            (Some(rebalance(root)), removed)
        }
        Ordering::Equal => {
            let Node {
                value, left, right, ..
            } = *root;
            match (left, right) {
                (None, right) => (right, Some(value)),
                (left, None) => (left, Some(value)),
                (Some(left), Some(right)) => {
                    let (right, mut successor) = take_min(right);
                    successor.left = Some(left);
                    successor.right = right;
                    (Some(rebalance(successor)), Some(value))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_avl<V>(node: &Link<V>) -> (usize, u8) {
        let Some(node) = node else { return (0, 0) };
        let (left_len, left_height) = assert_avl(&node.left);
        let (right_len, right_height) = assert_avl(&node.right);
        assert!(left_height.abs_diff(right_height) <= 1);
        assert_eq!(node.height, 1 + left_height.max(right_height));
        if let Some(left) = &node.left {
            assert!(left.key < node.key);
        }
        if let Some(right) = &node.right {
            assert!(node.key < right.key);
        }
        (1 + left_len + right_len, node.height)
    }

    #[test]
    fn ascending_insert_and_permuted_remove_preserve_avl_invariants() {
        const COUNT: usize = 1024;
        let mut table = OrderedTable::new(COUNT);
        for key in 0..COUNT as u64 {
            table.try_insert(key, key * 3).unwrap_or_else(|_| panic!());
        }
        assert_eq!(assert_avl(&table.root).0, COUNT);
        for key in 0..COUNT as u64 {
            assert_eq!(table.get(key), Some(&(key * 3)));
        }
        for step in 0..COUNT as u64 {
            let key = (step * 683) % COUNT as u64;
            assert_eq!(table.remove(key), Some(key * 3));
            assert_eq!(assert_avl(&table.root).0, COUNT - step as usize - 1);
        }
        assert!(table.is_empty());
    }

    #[test]
    fn prepared_entry_returns_value_and_limit_returns_input() {
        let mut table = OrderedTable::new(1);
        let prepared = table.prepare_insert(7, 70).unwrap_or_else(|_| panic!());
        assert_eq!(prepared.into_value(), 70);
        table.try_insert(8, 80).unwrap_or_else(|_| panic!());
        match table.try_insert(9, 90) {
            Err(InsertError::Limit(value)) => assert_eq!(value, 90),
            _ => panic!("capacity exhaustion returned the wrong result"),
        }
    }

    #[test]
    fn existing_candidate_survives_full_table_removal_race() {
        let mut table = OrderedTable::new(1);
        table.try_insert(7, 70).unwrap_or_else(|_| panic!());
        let candidate = table
            .prepare_insert_candidate(7, 71)
            .unwrap_or_else(|_| panic!());
        assert_eq!(table.remove(7), Some(70));
        table.insert_prepared(candidate);
        assert_eq!(table.get(7), Some(&71));

        let redundant = table
            .prepare_insert_candidate(7, 72)
            .unwrap_or_else(|_| panic!());
        assert_eq!(redundant.into_value(), 72);
        assert_eq!(table.get(7), Some(&71));
    }

    #[test]
    fn visible_scan_stops_at_transaction_barrier_and_resumes_by_cursor() {
        let mut table = OrderedTable::new(4);
        for (key, visible) in [(1, true), (2, true), (3, false), (4, true)] {
            table.try_insert(key, visible).unwrap_or_else(|_| panic!());
        }
        let mut out = [0; 4];
        assert_eq!(table.scan_visible(|value| *value, 0, &mut out), (2, true));
        assert_eq!(&out[..2], &[1, 2]);
        assert_eq!(table.scan_visible(|value| *value, 2, &mut out), (0, true));
        *table.get_mut(3).expect("barrier entry disappeared") = true;
        assert_eq!(table.scan_visible(|value| *value, 2, &mut out), (2, false));
        assert_eq!(&out[..2], &[3, 4]);
        assert_eq!(table.count_matching(|value| *value), 4);
    }
}
