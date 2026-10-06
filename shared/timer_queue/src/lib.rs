#![no_std]

//! 侵入式期限 AVL 树。节点出生时预付，登记/取消/到期弹出不分配；
//! 节点身份即稳定地址，不存在 arena token 或 generation。所有树访问由
//! 外部队列锁串行化，载荷由到期/取消方移出锁后处理。

use core::{
    cell::UnsafeCell,
    marker::PhantomPinned,
    pin::Pin,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

const DETACHED: usize = usize::MAX;

struct Links<T> {
    parent: *const TimerNode<T>,
    left: *const TimerNode<T>,
    right: *const TimerNode<T>,
    height: u16,
    deadline: u64,
    value: Option<T>,
}

pub struct TimerNode<T> {
    owner: AtomicUsize,
    links: UnsafeCell<Links<T>>,
    _pin: PhantomPinned,
}

impl<T> TimerNode<T> {
    pub const fn new() -> Self {
        Self {
            owner: AtomicUsize::new(DETACHED),
            links: UnsafeCell::new(Links {
                parent: ptr::null(),
                left: ptr::null(),
                right: ptr::null(),
                height: 1,
                deadline: 0,
                value: None,
            }),
            _pin: PhantomPinned,
        }
    }

    /// 仅用于选择需要锁住的 owner 队列；加锁后取消仍须复检。
    pub fn owner_slot(&self) -> Option<usize> {
        match self.owner.load(Ordering::Acquire) {
            DETACHED => None,
            owner => Some(owner),
        }
    }
}

impl<T> Default for TimerNode<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Drop for TimerNode<T> {
    fn drop(&mut self) {
        assert!(
            self.owner_slot().is_none() && self.links.get_mut().value.is_none(),
            "registered timer node was released"
        );
    }
}
// SAFETY: 链与载荷只在唯一 owner 队列锁下访问，owner 是队列选择用的原子字段。
unsafe impl<T: Send> Send for TimerNode<T> {}
unsafe impl<T: Send> Sync for TimerNode<T> {}

pub struct TimerQueue<T> {
    owner_slot: usize,
    root: *const TimerNode<T>,
    first: *const TimerNode<T>,
    len: usize,
}
// SAFETY: root 指向 pin 节点，全部树操作由外部锁串行化；未挂节点时可移动队列。
unsafe impl<T: Send> Send for TimerQueue<T> {}

impl<T> TimerQueue<T> {
    pub const fn unbound() -> Self {
        Self {
            owner_slot: DETACHED,
            root: ptr::null(),
            first: ptr::null(),
            len: 0,
        }
    }

    pub fn bind_owner(&mut self, owner_slot: usize) -> bool {
        assert!(owner_slot != DETACHED, "invalid timer owner");
        if self.owner_slot == DETACHED {
            assert!(self.is_empty(), "unbound timer queue retained nodes");
            self.owner_slot = owner_slot;
        }
        self.owner_slot == owner_slot
    }

    pub const fn new(owner_slot: usize) -> Self {
        assert!(owner_slot != DETACHED);
        Self {
            owner_slot,
            root: ptr::null(),
            first: ptr::null(),
            len: 0,
        }
    }

    /// # Safety
    /// 共享路由域内 owner_slot 唯一；私有 actor 只向所属队列传节点。
    /// 节点保持 pin/存活直至 cancel/pop_expired。
    /// 登记/取消/弹出必须在相应队列的同一锁下执行；未挂节点的初始化由
    /// 唯一执行 owner 控制。载荷通常持线程强根与等待 epoch，保证队列可达性。
    pub unsafe fn register(&mut self, node: Pin<&TimerNode<T>>, deadline: u64, value: T) {
        assert!(self.owner_slot != DETACHED, "unbound timer registration");
        let node = node.get_ref() as *const TimerNode<T>;
        assert!(
            unsafe { (*node).owner_slot().is_none() },
            "timer registered twice"
        );
        let mut parent = ptr::null();
        let mut current = self.root;
        while !current.is_null() {
            parent = current;
            // SAFETY: 树节点按登记契约保持存活，调用者持队列锁。
            let links = unsafe { &*(*current).links.get() };
            current = if (deadline, node as usize) < (links.deadline, current as usize) {
                links.left
            } else {
                links.right
            };
        }
        {
            let links = unsafe { &mut *(*node).links.get() };
            assert!(links.value.is_none(), "unlinked timer retained payload");
            links.parent = parent;
            links.left = ptr::null();
            links.right = ptr::null();
            links.height = 1;
            links.deadline = deadline;
            links.value = Some(value);
        }
        if parent.is_null() {
            self.root = node;
        } else {
            let links = unsafe { &mut *(*parent).links.get() };
            if (deadline, node as usize) < (links.deadline, parent as usize) {
                links.left = node;
            } else {
                links.right = node;
            }
        }
        self.len = self.len.checked_add(1).expect("timer node count exhausted");
        if self.first.is_null()
            || (deadline, node as usize)
                < unsafe { ((*(*self.first).links.get()).deadline, self.first as usize) }
        {
            self.first = node;
        }
        unsafe {
            (*node).owner.store(self.owner_slot, Ordering::Release);
            self.rebalance(parent);
        }
    }

    /// # Safety
    /// 调用者根据路由域或私有 owner 选到正确队列；同域队列的 slot 唯一。
    /// 全部操作持同一队列锁，节点的 pin storage 仍存活。
    /// 外部旧事件必须先按等待 epoch 仲裁，不能凭旧期限取消新轮登记。
    pub unsafe fn cancel(&mut self, node: Pin<&TimerNode<T>>) -> Option<T> {
        // SAFETY: 继承节点/队列同步域契约，无额外目标访问。
        unsafe { self.cancel_if(node, |_| true) }
    }

    /// # Safety
    /// 同 cancel；predicate 在队列锁内只检查载荷身份，不触及其他同步域。
    /// 内核用它复检等待 epoch，拒绝旧到期提示对复用节点的取消。
    pub unsafe fn cancel_if(
        &mut self,
        node: Pin<&TimerNode<T>>,
        predicate: impl FnOnce(&T) -> bool,
    ) -> Option<T> {
        let node = node.get_ref() as *const TimerNode<T>;
        if unsafe { (*node).owner_slot() } != Some(self.owner_slot) {
            return None;
        }
        let accepted = {
            let links = unsafe { &*(*node).links.get() };
            predicate(links.value.as_ref().expect("registered timer lost payload"))
        };
        if !accepted {
            return None;
        }
        Some(unsafe { self.remove(node) })
    }

    pub fn peek_expires_at(&self) -> Option<u64> {
        let node = self.first;
        if node.is_null() {
            None
        } else {
            Some(unsafe { (*(*node).links.get()).deadline })
        }
    }

    pub fn peek(&self) -> Option<(u64, &T)> {
        if self.first.is_null() {
            None
        } else {
            // SAFETY: first 是队列锁内更新的最小值缓存，登记契约保持节点存活。
            let links = unsafe { &*(*self.first).links.get() };
            Some((
                links.deadline,
                links.value.as_ref().expect("timer minimum lost payload"),
            ))
        }
    }

    /// 锁内彻底摘链、移出载荷；返回后不再持节点借用，允许线程重用内嵌节点。
    pub fn pop_expired(&mut self, now: u64) -> Option<T> {
        let node = self.first;
        if node.is_null() || unsafe { (*(*node).links.get()).deadline > now } {
            return None;
        }
        Some(unsafe { self.remove(node) })
    }

    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    unsafe fn minimum(mut node: *const TimerNode<T>) -> *const TimerNode<T> {
        // SAFETY: 只跟随登记契约下的树边，持同一队列锁。
        while !node.is_null() && !unsafe { (*(*node).links.get()).left }.is_null() {
            node = unsafe { (*(*node).links.get()).left };
        }
        node
    }

    unsafe fn height(node: *const TimerNode<T>) -> u16 {
        if node.is_null() {
            0
        } else {
            unsafe { (*(*node).links.get()).height }
        }
    }

    unsafe fn update_height(node: *const TimerNode<T>) {
        let (left, right) = unsafe { ((*(*node).links.get()).left, (*(*node).links.get()).right) };
        unsafe {
            (*(*node).links.get()).height = 1 + Self::height(left).max(Self::height(right));
        }
    }

    unsafe fn balance(node: *const TimerNode<T>) -> i32 {
        unsafe {
            i32::from(Self::height((*(*node).links.get()).left))
                - i32::from(Self::height((*(*node).links.get()).right))
        }
    }

    unsafe fn replace(&mut self, old: *const TimerNode<T>, new: *const TimerNode<T>) {
        let parent = unsafe { (*(*old).links.get()).parent };
        if parent.is_null() {
            self.root = new;
        } else if unsafe { (*(*parent).links.get()).left == old } {
            unsafe {
                (*(*parent).links.get()).left = new;
            }
        } else {
            assert!(
                unsafe { (*(*parent).links.get()).right == old },
                "timer parent lost child"
            );
            unsafe {
                (*(*parent).links.get()).right = new;
            }
        }
        if !new.is_null() {
            unsafe {
                (*(*new).links.get()).parent = parent;
            }
        }
    }

    unsafe fn rotate_left(&mut self, old: *const TimerNode<T>) -> *const TimerNode<T> {
        let new = unsafe { (*(*old).links.get()).right };
        assert!(!new.is_null(), "left timer rotation without right child");
        let middle = unsafe { (*(*new).links.get()).left };
        unsafe {
            self.replace(old, new);
            (*(*new).links.get()).left = old;
            (*(*old).links.get()).parent = new;
            (*(*old).links.get()).right = middle;
            if !middle.is_null() {
                (*(*middle).links.get()).parent = old;
            }
            Self::update_height(old);
            Self::update_height(new);
        }
        new
    }

    unsafe fn rotate_right(&mut self, old: *const TimerNode<T>) -> *const TimerNode<T> {
        let new = unsafe { (*(*old).links.get()).left };
        assert!(!new.is_null(), "right timer rotation without left child");
        let middle = unsafe { (*(*new).links.get()).right };
        unsafe {
            self.replace(old, new);
            (*(*new).links.get()).right = old;
            (*(*old).links.get()).parent = new;
            (*(*old).links.get()).left = middle;
            if !middle.is_null() {
                (*(*middle).links.get()).parent = old;
            }
            Self::update_height(old);
            Self::update_height(new);
        }
        new
    }

    unsafe fn rebalance(&mut self, mut node: *const TimerNode<T>) {
        // SAFETY: 每步沿父链上升，AVL 高度 O(log n)，旋转不移动节点/载荷。
        while !node.is_null() {
            unsafe {
                Self::update_height(node);
            }
            let mut root = node;
            if unsafe { Self::balance(node) } > 1 {
                let left = unsafe { (*(*node).links.get()).left };
                if unsafe { Self::balance(left) } < 0 {
                    unsafe {
                        self.rotate_left(left);
                    }
                }
                root = unsafe { self.rotate_right(node) };
            } else if unsafe { Self::balance(node) } < -1 {
                let right = unsafe { (*(*node).links.get()).right };
                if unsafe { Self::balance(right) } > 0 {
                    unsafe {
                        self.rotate_right(right);
                    }
                }
                root = unsafe { self.rotate_left(node) };
            }
            node = unsafe { (*(*root).links.get()).parent };
        }
    }

    unsafe fn remove(&mut self, node: *const TimerNode<T>) -> T {
        let (left, right) = unsafe { ((*(*node).links.get()).left, (*(*node).links.get()).right) };
        let repair;
        if left.is_null() || right.is_null() {
            repair = unsafe { (*(*node).links.get()).parent };
            unsafe {
                self.replace(node, if left.is_null() { right } else { left });
            }
        } else {
            let successor = unsafe { Self::minimum(right) };
            let previous_parent = unsafe { (*(*successor).links.get()).parent };
            if previous_parent == node {
                repair = successor;
            } else {
                repair = previous_parent;
                let child = unsafe { (*(*successor).links.get()).right };
                unsafe {
                    self.replace(successor, child);
                    (*(*successor).links.get()).right = right;
                    (*(*right).links.get()).parent = successor;
                }
            }
            unsafe {
                self.replace(node, successor);
                (*(*successor).links.get()).left = left;
                (*(*left).links.get()).parent = successor;
            }
        }
        self.len -= 1;
        unsafe {
            self.rebalance(repair);
        }
        if self.first == node {
            self.first = unsafe { Self::minimum(self.root) };
        }
        let value = {
            let links = unsafe { &mut *(*node).links.get() };
            links.parent = ptr::null();
            links.left = ptr::null();
            links.right = ptr::null();
            links.height = 1;
            links.value.take().expect("timer node lost payload")
        };
        unsafe {
            (*node).owner.store(DETACHED, Ordering::Release);
        }
        value
    }
}

impl<T> Drop for TimerQueue<T> {
    fn drop(&mut self) {
        assert!(
            self.root.is_null() && self.len == 0,
            "timer queue released with nodes attached"
        );
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::{boxed::Box, vec::Vec};

    // 检查 AVL 的独立平衡不变量，防止退化造成无界线性内核推进。
    fn invariant<T>(queue: &TimerQueue<T>) {
        unsafe fn visit<T>(node: *const TimerNode<T>, parent: *const TimerNode<T>) -> (u16, usize) {
            if node.is_null() {
                return (0, 0);
            }
            let links = unsafe { &*(*node).links.get() };
            assert_eq!(links.parent, parent);
            let (lh, ln) = unsafe { visit(links.left, node) };
            let (rh, rn) = unsafe { visit(links.right, node) };
            assert!(lh.abs_diff(rh) <= 1, "timer tree lost AVL balance");
            assert_eq!(links.height, 1 + lh.max(rh));
            (links.height, 1 + ln + rn)
        }
        let (_, count) = unsafe { visit(queue.root, ptr::null()) };
        assert_eq!(count, queue.len());
    }

    #[test]
    fn cancellation_preserves_addresses_and_sorted_expiry() {
        let nodes: Vec<_> = (0..128).map(|_| Box::pin(TimerNode::new())).collect();
        let mut queue = TimerQueue::new(0);
        // SAFETY: 所有 pin 节点保持存活到弹出/取消，队列单线程独占。
        for (id, node) in nodes.iter().enumerate() {
            unsafe {
                queue.register(node.as_ref(), (id * 37 % 128) as u64, id);
            }
            invariant(&queue);
        }
        let mut expected = Vec::new();
        for (id, node) in nodes.iter().enumerate() {
            if id % 3 == 0 {
                assert_eq!(unsafe { queue.cancel(node.as_ref()) }, Some(id));
                invariant(&queue);
            } else {
                expected.push((id * 37 % 128, id));
            }
        }
        expected.sort_unstable();
        for (deadline, id) in expected {
            assert_eq!(queue.peek_expires_at(), Some(deadline as u64));
            assert_eq!(queue.pop_expired(deadline as u64), Some(id));
            invariant(&queue);
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn expiry_detaches_before_reuse_and_wrong_owner_cannot_cancel() {
        let node = Box::pin(TimerNode::new());
        let mut first = TimerQueue::new(0);
        let mut second = TimerQueue::new(1);
        unsafe {
            first.register(node.as_ref(), 10, 7);
        }
        assert_eq!(unsafe { second.cancel(node.as_ref()) }, None);
        assert_eq!(first.pop_expired(9), None);
        let old_delivery = first.pop_expired(10);
        assert_eq!(node.owner_slot(), None);
        unsafe {
            second.register(node.as_ref(), 20, 8);
        }
        assert_eq!(unsafe { first.cancel(node.as_ref()) }, None);
        assert_eq!(old_delivery, Some(7));
        assert_eq!(second.pop_expired(20), Some(8));
        assert_eq!(unsafe { second.cancel(node.as_ref()) }, None);
        unsafe {
            second.register(node.as_ref(), 30, 9);
        }
        assert_eq!(
            unsafe { second.cancel_if(node.as_ref(), |identity| *identity == 8) },
            None
        );
        assert_eq!(second.pop_expired(30), Some(9));
    }

    #[test]
    fn equal_deadlines_keep_all_nodes_and_each_cancel_is_local() {
        let nodes: Vec<_> = (0..80).map(|_| Box::pin(TimerNode::new())).collect();
        let mut queue = TimerQueue::new(3);
        for (id, node) in nodes.iter().enumerate() {
            unsafe {
                queue.register(node.as_ref(), 50, id);
            }
        }
        let mut seen = [false; 80];
        while let Some(id) = queue.pop_expired(50) {
            assert!(!seen[id]);
            seen[id] = true;
            invariant(&queue);
        }
        assert!(seen.into_iter().all(|seen| seen));
    }
}
