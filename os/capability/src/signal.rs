//! 纯电平等待链：节点 owner 自摘，来源扫描只借用目标、不给等待写结果。
//!
//! 来源与节点须处于稳定地址，所有链访问由调用者同一同步域串行化。
//! 稳定边界标记隔开本轮与新增节点；删除游标节点在同域前移游标。

use core::{cell::UnsafeCell, marker::PhantomPinned, pin::Pin, ptr};

struct Links<T> {
    previous: *const Node<T>,
    next: *const Node<T>,
    owner: *const Source<T>,
    interest: u64,
    target: Option<T>,
}

/// 稳定节点，storage 的 owner 保持来源观察存储直到 remove 完成。
pub struct Node<T> {
    links: UnsafeCell<Links<T>>,
    _pin: PhantomPinned,
}

impl<T> Node<T> {
    pub const fn new() -> Self {
        Self {
            links: UnsafeCell::new(Links {
                previous: ptr::null(),
                next: ptr::null(),
                owner: ptr::null(),
                interest: 0,
                target: None,
            }),
            _pin: PhantomPinned,
        }
    }
}

impl<T> Default for Node<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for Node<T> {
    fn drop(&mut self) {
        assert!(
            self.links.get_mut().owner.is_null(),
            "linked signal node was released"
        );
    }
}

// SAFETY: 节点链接和 target 仅在其来源同步域下访问；未挂接时由唯一 owner 控制。
unsafe impl<T: Send> Send for Node<T> {}
unsafe impl<T: Send> Sync for Node<T> {}

/// 一步扫描的观察，不包含摘链或结果提交。
pub enum Scan<'a, T> {
    Candidate(&'a T),
    Skipped,
    /// 移动本轮边界，仍有后续扫描责任。
    Boundary,
    Idle,
}

pub struct Source<T> {
    head: *const Node<T>,
    tail: *const Node<T>,
    cursor: *const Node<T>,
    boundary: Node<T>,
    level: u64,
    closed: bool,
    requested: bool,
    _pin: PhantomPinned,
}

// SAFETY: 来源移动只发生在挂链前；Pin 后由外部同一同步域串行保护。
unsafe impl<T: Send> Send for Source<T> {}

impl<T> Source<T> {
    pub const fn new() -> Self {
        Self {
            head: ptr::null(),
            tail: ptr::null(),
            cursor: ptr::null(),
            boundary: Node::new(),
            level: 0,
            closed: false,
            requested: false,
            _pin: PhantomPinned,
        }
    }

    pub const fn level(&self) -> u64 {
        self.level
    }
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// 只改变电平与一位扫描责任，更新期间不重置游标。
    pub fn update(self: Pin<&mut Self>, level: u64, closed: bool) -> bool {
        // SAFETY: 不移动被 pin 的任何字段。
        let this = unsafe { self.get_unchecked_mut() };
        if this.level == level && this.closed == closed {
            return false;
        }
        this.level = level;
        this.closed = closed;
        this.requested = true;
        true
    }

    /// # Safety
    /// 节点保持 pin/存活直至 remove，同一来源同步域独占其全部链访问；
    /// target 仅包含观察身份，内核消费者应为 Weak，不得形成来源→线程强持。
    pub unsafe fn insert(self: Pin<&mut Self>, node: Pin<&Node<T>>, interest: u64, target: T) {
        // SAFETY: 由调用者保持地址和同步域，未移动字段。
        let this = unsafe { self.get_unchecked_mut() };
        let owner = this as *const Self;
        let boundary = &this.boundary as *const Node<T>;
        if this.head.is_null() {
            // SAFETY: boundary 内嵌于 pin 来源，链接只在本同步域访问。
            unsafe {
                (*boundary).links.get().as_mut().unwrap().owner = owner;
            }
            this.head = boundary;
            this.tail = boundary;
        }
        let node = node.get_ref() as *const Node<T>;
        // SAFETY: 调用方保证稳定节点和独占链接访问。
        let links = unsafe { &mut *(*node).links.get() };
        assert!(
            links.owner.is_null() && links.target.is_none(),
            "signal node linked twice"
        );
        links.owner = owner;
        links.interest = interest;
        links.target = Some(target);
        // 新节点在边界之后，不插进本轮未扫描条目。
        unsafe {
            this.append(node);
        }
    }

    /// # Safety
    /// 同 insert 的存储与同步域契约；节点必须由本来源挂接。
    pub unsafe fn remove(self: Pin<&mut Self>, node: Pin<&Node<T>>) -> T {
        let this = unsafe { self.get_unchecked_mut() };
        let node = node.get_ref() as *const Node<T>;
        let next = {
            let links = unsafe { &*(*node).links.get() };
            assert!(
                ptr::eq(links.owner, this),
                "signal node detached from another source"
            );
            links.next
        };
        if this.cursor == node {
            this.cursor = next;
        }
        unsafe {
            this.unlink(node);
        }
        let links = unsafe { &mut *(*node).links.get() };
        links.owner = ptr::null();
        links
            .target
            .take()
            .expect("signal node lost its observation identity")
    }

    /// 每次只检查一个节点或移动一个边界，返回的借用不允许跨来源解锁。
    pub fn step(self: Pin<&mut Self>) -> Scan<'_, T> {
        let this = unsafe { self.get_unchecked_mut() };
        if this.cursor.is_null() {
            if !this.requested {
                return Scan::Idle;
            }
            this.requested = false;
            if !this.head.is_null() {
                // SAFETY: 新一轮开始前，把间歇期间新增条目包含到本轮。
                let boundary = &this.boundary as *const Node<T>;
                unsafe {
                    this.unlink(boundary);
                    this.append(boundary);
                }
            }
            this.cursor = this.head;
        }
        if this.cursor.is_null() {
            return Scan::Idle;
        }
        let boundary = &this.boundary as *const Node<T>;
        if this.cursor == boundary {
            // SAFETY: 内嵌边界稳定且在本链，本轮旧节点已处理完。
            unsafe {
                this.unlink(boundary);
                this.append(boundary);
            }
            this.cursor = ptr::null();
            return if this.requested {
                Scan::Boundary
            } else {
                Scan::Idle
            };
        }
        let node = this.cursor;
        // SAFETY: insert 保证链节点存活；remove 在同同步域前移游标。
        let links = unsafe { &*(*node).links.get() };
        this.cursor = links.next;
        if this.closed || this.level & links.interest != 0 {
            Scan::Candidate(links.target.as_ref().expect("signal candidate lost target"))
        } else {
            Scan::Skipped
        }
    }

    unsafe fn append(&mut self, node: *const Node<T>) {
        let links = unsafe { &mut *(*node).links.get() };
        links.previous = self.tail;
        links.next = ptr::null();
        if self.tail.is_null() {
            self.head = node;
        } else {
            unsafe {
                (*(*self.tail).links.get()).next = node;
            }
        }
        self.tail = node;
    }

    unsafe fn unlink(&mut self, node: *const Node<T>) {
        let links = unsafe { &mut *(*node).links.get() };
        if links.previous.is_null() {
            self.head = links.next;
        } else {
            unsafe {
                (*(*links.previous).links.get()).next = links.next;
            }
        }
        if links.next.is_null() {
            self.tail = links.previous;
        } else {
            unsafe {
                (*(*links.next).links.get()).previous = links.previous;
            }
        }
        links.previous = ptr::null();
        links.next = ptr::null();
    }
}

impl<T> Default for Source<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Drop for Source<T> {
    fn drop(&mut self) {
        let boundary = &self.boundary as *const Node<T>;
        assert!(
            self.head.is_null() || (self.head == boundary && self.tail == boundary),
            "signal source released with observers attached"
        );
        self.boundary.links.get_mut().owner = ptr::null();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, vec::Vec};

    fn round(source: Pin<&mut Source<usize>>) -> Vec<usize> {
        let mut source = source;
        let mut seen = Vec::new();
        for _ in 0..100 {
            match source.as_mut().step() {
                Scan::Candidate(id) => seen.push(*id),
                Scan::Skipped | Scan::Boundary => {}
                Scan::Idle => return seen,
            }
        }
        panic!("bounded signal round failed to terminate");
    }

    #[test]
    fn insertion_is_after_the_boundary_and_owner_rechecks_new_nodes() {
        let mut source = core::pin::pin!(Source::new());
        let a = Box::pin(Node::new());
        let b = Box::pin(Node::new());
        // SAFETY: 来源和节点均保持 pin，直到从同一来源摘除；没有并发访问。
        unsafe {
            source.as_mut().insert(a.as_ref(), 1, 1);
        }
        source.as_mut().update(1, false);
        assert_eq!(round(source.as_mut()), [1]);
        source.as_mut().update(3, false);
        assert!(matches!(source.as_mut().step(), Scan::Candidate(&1)));
        unsafe {
            source.as_mut().insert(b.as_ref(), 1, 2);
        }
        assert!(round(source.as_mut()).is_empty());
        source.as_mut().update(1, false);
        assert_eq!(round(source.as_mut()), [1, 2]);
        unsafe {
            source.as_mut().remove(a.as_ref());
            source.as_mut().remove(b.as_ref());
        }
    }

    #[test]
    fn deleting_cursor_and_updating_during_scan_preserve_remaining_observers() {
        let mut source = core::pin::pin!(Source::new());
        let a = Box::pin(Node::new());
        let b = Box::pin(Node::new());
        let c = Box::pin(Node::new());
        unsafe {
            source.as_mut().insert(a.as_ref(), 1, 1);
            source.as_mut().insert(b.as_ref(), 1, 2);
            source.as_mut().insert(c.as_ref(), 1, 3);
        }
        source.as_mut().update(1, false);
        round(source.as_mut());
        source.as_mut().update(3, false);
        assert!(matches!(source.as_mut().step(), Scan::Candidate(&1)));
        unsafe {
            source.as_mut().remove(b.as_ref());
        }
        source.as_mut().update(1, false);
        assert_eq!(round(source.as_mut()), [3, 1, 3]);
        unsafe {
            source.as_mut().remove(a.as_ref());
            source.as_mut().remove(c.as_ref());
        }
    }

    #[test]
    fn closed_hints_all_interests_and_transient_levels_are_not_latched() {
        let mut source = core::pin::pin!(Source::new());
        let a = Box::pin(Node::new());
        unsafe {
            source.as_mut().insert(a.as_ref(), 4, 1);
        }
        source.as_mut().update(4, false);
        source.as_mut().update(0, false);
        assert!(round(source.as_mut()).is_empty());
        source.as_mut().update(0, true);
        assert_eq!(round(source.as_mut()), [1]);
        unsafe {
            source.as_mut().remove(a.as_ref());
        }
    }
}
