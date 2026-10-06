#![no_std]
#![forbid(unsafe_code)]

//! 带容量上限和可失败节点准备的 AVL 有序表。
//!
//! 节点只在准备/插入期分配；查找、修改和删除不分配。每个实例携带明确条目
//! 上限，因此树高和单次操作的比较次数都有硬上界。

extern crate alloc;

use core::{borrow::Borrow, cmp::Ordering};
use funded_storage::{Funding, OwnedBox, StorageError};

type NodeOwner<V, K, C> = OwnedBox<Node<V, K, C>, C>;
type Link<V, K, C> = Option<NodeOwner<V, K, C>>;
type Removal<V, K, C> = (Link<V, K, C>, Option<PreparedEntry<V, C, K>>);

struct Node<V, K, C> {
    key: K,
    value: V,
    height: u8,
    left: Link<V, K, C>,
    right: Link<V, K, C>,
}

pub struct PreparedEntry<V, C, K = u64>(NodeOwner<V, K, C>);

impl<V, C, K> PreparedEntry<V, C, K> {
    pub fn key(&self) -> K
    where
        K: Copy,
    {
        self.0.key
    }

    pub fn key_ref(&self) -> &K {
        &self.0.key
    }

    /// 身份由外部注册步骤才确定时复用预付节点；插入仍验证最终 key。
    pub fn with_key(mut self, key: K) -> Self {
        self.0.key = key;
        self
    }

    pub const fn allocation_bytes() -> usize {
        core::mem::size_of::<Node<V, K, C>>()
    }

    pub fn value_mut(&mut self) -> &mut V {
        &mut self.0.value
    }

    /// 完整节点带出外层锁后才解构；释放分配再退付款。
    pub fn into_parts(self) -> (K, V) {
        let Node {
            key,
            value,
            left,
            right,
            ..
        } = self.0.into_inner();
        assert!(
            left.is_none() && right.is_none(),
            "detached entry retained tree links"
        );
        (key, value)
    }
    pub fn into_value(self) -> V {
        self.into_parts().1
    }
    pub fn value(&self) -> &V {
        &self.0.value
    }
    pub fn try_new<F: Funding<Charge = C>>(
        key: K,
        value: V,
        payer: &F,
    ) -> Result<Self, InsertError<V, F::Error>> {
        OwnedBox::try_new(
            Node {
                key,
                value,
                height: 1,
                left: None,
                right: None,
            },
            payer,
        )
        .map(Self)
        .map_err(|(error, node)| InsertError::Storage(error, node.value))
    }
}

pub enum InsertError<V, E> {
    Limit(V),
    Storage(StorageError<E>, V),
}

pub struct OrderedTable<V, C, K = u64> {
    root: Link<V, K, C>,
    len: usize,
    limit: usize,
}

impl<V, C, K: Ord> OrderedTable<V, C, K> {
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

    pub fn has_room(&self) -> bool {
        self.len < self.limit
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn prepare_insert<F: Funding<Charge = C>>(
        &self,
        key: K,
        value: V,
        payer: &F,
    ) -> Result<PreparedEntry<V, C, K>, InsertError<V, F::Error>> {
        if self.len == self.limit {
            return Err(InsertError::Limit(value));
        }
        if self.get_by(&key).is_some() {
            unreachable!("ordered table key prepared twice");
        }
        PreparedEntry::try_new(key, value, payer)
    }

    /// 为 Commit 时才确定“复用现项或插入新项”的事务准备候选节点。
    /// key 当前存在时即使表已满也允许准备：若该项在 Commit 前被移除，
    /// 空出的同一容量正好供候选插入；若仍存在，调用方丢弃候选即可。
    pub fn prepare_insert_candidate<F: Funding<Charge = C>>(
        &self,
        key: K,
        value: V,
        payer: &F,
    ) -> Result<PreparedEntry<V, C, K>, InsertError<V, F::Error>> {
        if self.get_by(&key).is_none() && self.len == self.limit {
            return Err(InsertError::Limit(value));
        }
        PreparedEntry::try_new(key, value, payer)
    }

    pub fn insert_prepared(&mut self, entry: PreparedEntry<V, C, K>) {
        assert!(
            self.len < self.limit,
            "ordered table capacity changed after preparation"
        );
        assert!(
            self.get_by(&entry.0.key).is_none(),
            "ordered table key changed after preparation"
        );
        self.root = Some(insert_node(self.root.take(), entry.0));
        self.len += 1;
    }

    pub fn try_insert<F: Funding<Charge = C>>(
        &mut self,
        key: K,
        value: V,
        payer: &F,
    ) -> Result<(), InsertError<V, F::Error>> {
        let entry = self.prepare_insert(key, value, payer)?;
        self.insert_prepared(entry);
        Ok(())
    }

    pub fn contains_key(&self, key: K) -> bool {
        self.get_by(&key).is_some()
    }

    /// 最大的 <= key 项；不扫描前驱链，查询成本由 AVL 高度界定。
    pub fn floor(&self, key: &K) -> Option<(&K, &V)> {
        let mut node = self.root.as_deref();
        let mut found = None;
        while let Some(current) = node {
            if current.key <= *key {
                found = Some((&current.key, &current.value));
                node = current.right.as_deref();
            } else {
                node = current.left.as_deref();
            }
        }
        found
    }

    /// 最小的 >= key 项，供已删左端片段的退休游标重取当前最小值。
    pub fn lower_bound(&self, key: &K) -> Option<(&K, &V)> {
        let mut node = self.root.as_deref();
        let mut found = None;
        while let Some(current) = node {
            if current.key >= *key {
                found = Some((&current.key, &current.value));
                node = current.left.as_deref();
            } else {
                node = current.right.as_deref();
            }
        }
        found
    }

    pub fn get(&self, key: K) -> Option<&V> {
        self.get_by(&key)
    }

    pub fn get_by<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let mut current = self.root.as_deref();
        while let Some(node) = current {
            match key.cmp(node.key.borrow()) {
                Ordering::Less => current = node.left.as_deref(),
                Ordering::Greater => current = node.right.as_deref(),
                Ordering::Equal => return Some(&node.value),
            }
        }
        None
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.get_mut_by(&key)
    }

    pub fn get_mut_by<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let mut current = self.root.as_deref_mut();
        while let Some(node) = current {
            match key.cmp(node.key.borrow()) {
                Ordering::Less => current = node.left.as_deref_mut(),
                Ordering::Greater => current = node.right.as_deref_mut(),
                Ordering::Equal => return Some(&mut node.value),
            }
        }
        None
    }

    pub fn remove(&mut self, key: K) -> Option<PreparedEntry<V, C, K>> {
        self.remove_by(&key)
    }

    pub fn remove_by<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<PreparedEntry<V, C, K>>
    where
        K: Borrow<Q>,
    {
        let (root, removed) = remove_node(self.root.take(), key);
        self.root = root;
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    /// 从借用游标取得下一项，不复制或分配名字；None 游标从最小项开始。
    pub fn next_after<Q: Ord + ?Sized>(&self, cursor: Option<&Q>) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
    {
        let mut current = self.root.as_deref();
        let mut next = None;
        while let Some(node) = current {
            if cursor.is_none_or(|cursor| node.key.borrow() > cursor) {
                next = Some((&node.key, &node.value));
                current = node.left.as_deref();
            } else {
                current = node.right.as_deref();
            }
        }
        next
    }

    /// 提取最小项和原始 key，不复制、不分配；适合有界容器退休。
    pub fn pop_first(&mut self) -> Option<PreparedEntry<V, C, K>> {
        let root = self.root.take()?;
        let (root, mut minimum) = take_min(root);
        self.root = root;
        self.len -= 1;
        minimum.height = 1;
        Some(PreparedEntry(minimum))
    }

    pub fn count_matching(&self, predicate: impl Fn(&V) -> bool) -> usize {
        fn count<V, K, C>(node: &Link<V, K, C>, predicate: &impl Fn(&V) -> bool) -> usize {
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
        cursor: K,
        out: &mut [K],
    ) -> (usize, bool)
    where
        K: Copy,
    {
        fn scan<V, K: Ord + Copy, C>(
            node: &Link<V, K, C>,
            visible: &impl Fn(&V) -> bool,
            cursor: K,
            out: &mut [K],
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

fn height<V, K, C>(node: &Link<V, K, C>) -> u8 {
    node.as_ref().map_or(0, |node| node.height)
}

fn update_height<V, K, C>(node: &mut Node<V, K, C>) {
    node.height = 1 + height(&node.left).max(height(&node.right));
}

fn rotate_left<V, K, C>(mut root: NodeOwner<V, K, C>) -> NodeOwner<V, K, C> {
    let mut pivot = root.right.take().expect("AVL left rotation lost pivot");
    root.right = pivot.left.take();
    update_height(&mut root);
    pivot.left = Some(root);
    update_height(&mut pivot);
    pivot
}

fn rotate_right<V, K, C>(mut root: NodeOwner<V, K, C>) -> NodeOwner<V, K, C> {
    let mut pivot = root.left.take().expect("AVL right rotation lost pivot");
    root.left = pivot.right.take();
    update_height(&mut root);
    pivot.right = Some(root);
    update_height(&mut pivot);
    pivot
}

fn rebalance<V, K, C>(mut node: NodeOwner<V, K, C>) -> NodeOwner<V, K, C> {
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

fn insert_node<V, K: Ord, C>(root: Link<V, K, C>, node: NodeOwner<V, K, C>) -> NodeOwner<V, K, C> {
    let Some(mut root) = root else { return node };
    match node.key.cmp(&root.key) {
        Ordering::Less => root.left = Some(insert_node(root.left.take(), node)),
        Ordering::Greater => root.right = Some(insert_node(root.right.take(), node)),
        Ordering::Equal => unreachable!("ordered table key inserted twice"),
    }
    rebalance(root)
}

fn take_min<V, K, C>(mut node: NodeOwner<V, K, C>) -> (Link<V, K, C>, NodeOwner<V, K, C>) {
    let Some(left) = node.left.take() else {
        return (node.right.take(), node);
    };
    let (new_left, minimum) = take_min(left);
    node.left = new_left;
    (Some(rebalance(node)), minimum)
}

fn remove_node<V, K: Borrow<Q>, Q: Ord + ?Sized, C>(
    root: Link<V, K, C>,
    key: &Q,
) -> Removal<V, K, C> {
    let Some(mut root) = root else {
        return (None, None);
    };
    match key.cmp(root.key.borrow()) {
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
            let left = root.left.take();
            let right = root.right.take();
            root.height = 1;
            let removed = PreparedEntry(root);
            match (left, right) {
                (None, right) => (right, Some(removed)),
                (left, None) => (left, Some(removed)),
                (Some(left), Some(right)) => {
                    let (right, mut successor) = take_min(right);
                    successor.left = Some(left);
                    successor.right = right;
                    (Some(rebalance(successor)), Some(removed))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::Cell;

    struct Charge(Rc<Cell<usize>>);
    impl Drop for Charge {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }
    struct Meter {
        live: Rc<Cell<usize>>,
        deny: Cell<bool>,
    }
    impl Funding for Meter {
        type Charge = Charge;
        type Error = ();
        fn charge(&self, _: core::alloc::Layout) -> Result<Charge, ()> {
            if self.deny.get() {
                return Err(());
            }
            self.live.set(self.live.get() + 1);
            Ok(Charge(self.live.clone()))
        }
    }

    #[test]
    fn rejected_funding_returns_input_without_publishing_or_charging() {
        let payer = Meter {
            live: Default::default(),
            deny: Cell::new(true),
        };
        let mut table = OrderedTable::<u64, Charge>::new(4);
        assert!(matches!(
            table.try_insert(1, 7, &payer),
            Err(InsertError::Storage(StorageError::Funding(()), 7))
        ));
        assert!(table.is_empty());
        assert_eq!(payer.live.get(), 0);
    }

    #[test]
    fn two_child_removal_and_cross_table_move_keep_each_allocation_owner() {
        let payer = Meter {
            live: Default::default(),
            deny: Cell::new(false),
        };
        let mut table = OrderedTable::new(8);
        for key in [4, 2, 6, 1, 3, 5, 7] {
            table
                .try_insert(key, key, &payer)
                .unwrap_or_else(|_| panic!("paid insert failed"));
        }
        let removed = table.remove(4).expect("paid root missing");
        assert_eq!(removed.key(), 4);
        assert_eq!(*removed.value(), 4);
        assert_eq!(payer.live.get(), 7);
        for key in [1, 2, 3, 5, 6, 7] {
            assert_eq!(table.get(key), Some(&key));
        }
        drop(removed);
        assert_eq!(payer.live.get(), 6);
        let first = table.pop_first().expect("paid minimum missing");
        assert_eq!(first.key(), 1);
        assert_eq!(payer.live.get(), 6);
        let mut other = OrderedTable::new(2);
        other.insert_prepared(first.with_key(9));
        assert_eq!(payer.live.get(), 6);
        assert_eq!(other.get(9), Some(&1));
        drop(table);
        assert_eq!(payer.live.get(), 1);
        drop(other);
        assert_eq!(payer.live.get(), 0);
    }
    struct HeapFunding;
    impl Funding for HeapFunding {
        type Charge = ();
        type Error = ();
        fn charge(&self, _: core::alloc::Layout) -> Result<(), ()> {
            Ok(())
        }
    }

    #[test]
    fn bounds_include_exact_keys_and_skip_deleted_gaps() {
        let mut table = OrderedTable::new(4);
        for key in [40, 10, 30, 20] {
            table
                .try_insert(key, key, &HeapFunding)
                .unwrap_or_else(|_| panic!());
        }
        assert_eq!(table.floor(&0), None);
        assert_eq!(table.lower_bound(&50), None);
        assert_eq!(table.floor(&20), Some((&20, &20)));
        assert_eq!(table.lower_bound(&20), Some((&20, &20)));
        table.remove(20).map(PreparedEntry::into_value);
        assert_eq!(table.floor(&20), Some((&10, &10)));
        assert_eq!(table.lower_bound(&20), Some((&30, &30)));
        assert_eq!(table.floor(&29), Some((&10, &10)));
        assert_eq!(table.lower_bound(&31), Some((&40, &40)));
    }

    fn assert_avl<V>(node: &Link<V, u64, ()>) -> (usize, u8) {
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
            table
                .try_insert(key, key * 3, &HeapFunding)
                .unwrap_or_else(|_| panic!());
        }
        assert_eq!(assert_avl(&table.root).0, COUNT);
        for key in 0..COUNT as u64 {
            assert_eq!(table.get(key), Some(&(key * 3)));
        }
        for step in 0..COUNT as u64 {
            let key = (step * 683) % COUNT as u64;
            assert_eq!(
                table.remove(key).map(PreparedEntry::into_value),
                Some(key * 3)
            );
            assert_eq!(assert_avl(&table.root).0, COUNT - step as usize - 1);
        }
        assert!(table.is_empty());
    }

    #[test]
    fn owned_names_support_borrowed_lookup_cursor_and_removal() {
        use alloc::string::String;
        let mut table = OrderedTable::<u64, (), String>::new(4);
        table
            .try_insert(String::from("beta"), 2, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        table
            .try_insert(String::from("alpha"), 1, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        table
            .try_insert(String::from("gamma"), 3, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        assert_eq!(table.get_by("beta"), Some(&2));
        assert_eq!(
            table
                .next_after::<str>(None)
                .map(|(name, value)| (name.as_str(), *value)),
            Some(("alpha", 1))
        );
        assert_eq!(
            table
                .next_after(Some("beta"))
                .map(|(name, value)| (name.as_str(), *value)),
            Some(("gamma", 3))
        );
        *table.get_mut_by("beta").unwrap() = 20;
        assert_eq!(
            table.remove_by("beta").map(PreparedEntry::into_value),
            Some(20)
        );
        assert_eq!(table.get_by("beta"), None);
    }

    #[test]
    fn prepared_entry_returns_value_and_limit_returns_input() {
        let mut table = OrderedTable::new(1);
        let prepared = table
            .prepare_insert(7, 70, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        assert_eq!(prepared.into_value(), 70);
        table
            .try_insert(8, 80, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        match table.try_insert(9, 90, &HeapFunding) {
            Err(InsertError::Limit(value)) => assert_eq!(value, 90),
            _ => panic!("capacity exhaustion returned the wrong result"),
        }
    }

    #[test]
    fn existing_candidate_survives_full_table_removal_race() {
        let mut table = OrderedTable::new(1);
        table
            .try_insert(7, 70, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        let candidate = table
            .prepare_insert_candidate(7, 71, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        assert_eq!(table.remove(7).map(PreparedEntry::into_value), Some(70));
        table.insert_prepared(candidate);
        assert_eq!(table.get(7), Some(&71));

        let redundant = table
            .prepare_insert_candidate(7, 72, &HeapFunding)
            .unwrap_or_else(|_| panic!());
        assert_eq!(redundant.into_value(), 72);
        assert_eq!(table.get(7), Some(&71));
    }

    #[test]
    fn visible_scan_stops_at_transaction_barrier_and_resumes_by_cursor() {
        let mut table = OrderedTable::new(4);
        for (key, visible) in [(1, true), (2, true), (3, false), (4, true)] {
            table
                .try_insert(key, visible, &HeapFunding)
                .unwrap_or_else(|_| panic!());
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
