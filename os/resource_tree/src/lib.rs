#![no_std]
#![forbid(unsafe_code)]

//! 特权资源的层级准入与关闭账本，不解释硬件停止条件。
//!
//! 调用者以每根一把门串行化全部访问，并在同门提交实际成员/许可。
//! 祖先检查每步一个节点；门内不分配或退款。节点只有数字链接，无递归 owner。

use funded_storage::{Funding, StorageError};
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

#[cfg(test)]
mod tests;

static NEXT_NODE: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(u64);
impl NodeId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Missing,
    Closed,
    Busy,
    Incomplete,
    Capacity,
    WrongRoot,
}

pub enum PrepareError<V, E> {
    Identity(V),
    Storage(StorageError<E>, V),
}

struct Node<V> {
    parent: Option<NodeId>,
    previous: Option<NodeId>,
    next: Option<NodeId>,
    head: Option<NodeId>,
    tail: Option<NodeId>,
    revoke_next: Option<NodeId>,
    members: usize,
    sealed: bool,
    revoking: bool,
    finished: bool,
    value: V,
}

/// 节点与实际付款在门外准备；失败候选及摘除节点也必须带出门外释放。
pub struct PreparedNode<V, C>(PreparedEntry<Node<V>, C>);
/// 退休存储不能重新作为准备节点插回树，避免复活身份或改变原父关系。
pub struct RetiredNode<V, C>(PreparedEntry<Node<V>, C>);
impl<V, C> RetiredNode<V, C> {
    pub fn into_value(self) -> V {
        self.0.into_value().value
    }
}
impl<V, C> PreparedNode<V, C> {
    pub fn try_new<F: Funding<Charge = C>>(
        value: V,
        funding: &F,
    ) -> Result<Self, PrepareError<V, F::Error>> {
        let Some(id) = NEXT_NODE.allocate() else {
            return Err(PrepareError::Identity(value));
        };
        PreparedEntry::try_new(
            id,
            Node {
                parent: None,
                previous: None,
                next: None,
                head: None,
                tail: None,
                revoke_next: None,
                members: 0,
                sealed: false,
                revoking: false,
                finished: false,
                value,
            },
            funding,
        )
        .map(Self)
        .map_err(|error| match error {
            InsertError::Storage(error, node) => PrepareError::Storage(error, node.value),
            InsertError::Limit(_) => unreachable!("unbounded node preparation reached table limit"),
        })
    }

    pub fn id(&self) -> NodeId {
        NodeId(self.0.key())
    }

    pub fn value_mut(&mut self) -> &mut V {
        &mut self.0.value_mut().value
    }

    pub fn into_value(self) -> V {
        self.0.into_value().value
    }
}

/// 不能伪造或换根；可跨调度轮次保留，最终仍须 validate 后才提交实际许可。
pub struct Admission {
    root: NodeId,
    subject: NodeId,
    sequence: u64,
    next: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Pending,
    Checked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub parent: Option<NodeId>,
    pub sealed: bool,
    pub revoking: bool,
    pub finished: bool,
    pub members: usize,
    pub has_children: bool,
}

/// 一根一表。成员的具体记录由领域 owner 持有，计数与记录的增减在同门完成。
pub struct ResourceTree<V, C> {
    root: NodeId,
    sequence: Option<u64>,
    nodes: OrderedTable<Node<V>, C>,
}
impl<V, C> ResourceTree<V, C> {
    pub fn new(root: PreparedNode<V, C>) -> Self {
        let id = root.id();
        let mut nodes = OrderedTable::new(usize::MAX);
        nodes.insert_prepared(root.0);
        Self {
            root: id,
            sequence: Some(0),
            nodes,
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn value(&self, id: NodeId) -> Option<&V> {
        self.nodes.get(id.0).map(|node| &node.value)
    }

    pub fn value_mut(&mut self, id: NodeId) -> Option<&mut V> {
        self.nodes.get_mut(id.0).map(|node| &mut node.value)
    }

    pub fn status(&self, id: NodeId) -> Result<Status, Error> {
        let node = self.node(id)?;
        Ok(Status {
            parent: node.parent,
            sealed: node.sealed,
            revoking: node.revoking,
            finished: node.finished,
            members: node.members,
            has_children: node.head.is_some(),
        })
    }

    fn node(&self, id: NodeId) -> Result<&Node<V>, Error> {
        self.nodes.get(id.0).ok_or(Error::Missing)
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node<V> {
        self.nodes
            .get_mut(id.0)
            .expect("resource tree lost a linked node")
    }

    pub fn begin(&self, subject: NodeId) -> Result<Admission, Error> {
        if self.node(subject)?.sealed {
            return Err(Error::Closed);
        }
        Ok(Admission {
            root: self.root,
            subject,
            sequence: self.sequence.ok_or(Error::Closed)?,
            next: Some(subject),
        })
    }

    fn check_sequence(&self, admission: &Admission) -> Result<(), Error> {
        if admission.root != self.root {
            return Err(Error::WrongRoot);
        }
        match self.sequence {
            None => Err(Error::Closed),
            Some(current) if current != admission.sequence => Err(Error::Busy),
            Some(_) => Ok(()),
        }
    }

    /// 每次至多检查一个不可变 parent；变化后明确拒绝，不在门内自旋重试。
    pub fn advance(&self, admission: &mut Admission) -> Result<Progress, Error> {
        self.check_sequence(admission)?;
        if let Some(next) = admission.next {
            let node = self.node(next)?;
            if node.sealed {
                return Err(Error::Closed);
            }
            admission.next = node.parent;
        }
        Ok(if admission.next.is_none() {
            Progress::Checked
        } else {
            Progress::Pending
        })
    }

    /// 必须与领域的发布许可/插入在同一外部门内调用。
    pub fn validate(&self, admission: &Admission) -> Result<NodeId, Error> {
        self.check_sequence(admission)?;
        if admission.next.is_some() {
            return Err(Error::Incomplete);
        }
        Ok(admission.subject)
    }

    /// 成功只移动预付节点；错误将候选完整交回，禁止门内退款。
    pub fn link_child(
        &mut self,
        admission: &Admission,
        mut child: PreparedNode<V, C>,
    ) -> Result<NodeId, (Error, PreparedNode<V, C>)> {
        let parent = match self.validate(admission) {
            Ok(parent) => parent,
            Err(error) => return Err((error, child)),
        };
        if !self.nodes.has_room() {
            return Err((Error::Capacity, child));
        }
        let id = child.id();
        let tail = self.node_mut(parent).tail;
        child.0.value_mut().parent = Some(parent);
        child.0.value_mut().previous = tail;
        if let Some(tail) = tail {
            self.node_mut(tail).next = Some(id);
        } else {
            self.node_mut(parent).head = Some(id);
        }
        self.node_mut(parent).tail = Some(id);
        self.nodes.insert_prepared(child.0);
        Ok(id)
    }

    pub fn add_member(&mut self, admission: &Admission) -> Result<NodeId, Error> {
        let id = self.validate(admission)?;
        let node = self.node_mut(id);
        node.members = node.members.checked_add(1).ok_or(Error::Capacity)?;
        Ok(id)
    }

    /// 仅在实际责任安全结束后调用一次；由领域记录防止重复回执。
    pub fn finish_member(&mut self, id: NodeId) {
        let node = self.node_mut(id);
        node.members = node
            .members
            .checked_sub(1)
            .expect("resource member returned twice");
    }

    fn advance_sequence(&mut self) {
        self.sequence = self.sequence.and_then(|sequence| sequence.checked_add(1));
    }

    /// 封口不等同于撤销已有成员，序号耗尽永久拒绝全根的新准入。
    pub fn seal(&mut self, id: NodeId) -> Result<(), Error> {
        if !self.node(id)?.sealed {
            self.node_mut(id).sealed = true;
            self.advance_sequence();
        }
        Ok(())
    }

    pub fn revoke(&mut self, id: NodeId) -> Result<(), Error> {
        let node = self.node(id)?;
        if node.revoking || node.finished {
            return Ok(());
        }
        let node = self.node_mut(id);
        node.sealed = true;
        node.revoking = true;
        node.revoke_next = node.head;
        self.advance_sequence();
        Ok(())
    }

    /// 已封口的直接子链每次交付一个节点。完成子项可以并发摘链，不使游标悬空。
    /// 调用者在锁外把返回节点移交其预付清理 owner，不能丢弃这次移交。
    pub fn next_child_to_revoke(&mut self, id: NodeId) -> Result<Option<NodeId>, Error> {
        let node = self.node(id)?;
        if !node.revoking {
            return Err(Error::Incomplete);
        }
        let next = node.revoke_next;
        if let Some(next) = next {
            self.node_mut(id).revoke_next = self.node(next)?.next;
        }
        Ok(next)
    }

    /// 领域确认自己的停止条件后调用；本层另外要求子项/成员全部回执。
    /// 首次成功返回父节点以触发其复检；重复完成不再交父回执。
    pub fn finish(&mut self, id: NodeId) -> Result<Option<NodeId>, Error> {
        let node = self.node(id)?;
        if node.finished {
            return Ok(None);
        }
        if !node.sealed || node.head.is_some() || node.members != 0 {
            return Err(Error::Incomplete);
        }
        let (parent, previous, next) = (node.parent, node.previous, node.next);
        if let Some(parent) = parent {
            if let Some(previous) = previous {
                self.node_mut(previous).next = next;
            } else {
                self.node_mut(parent).head = next;
            }
            if let Some(next) = next {
                self.node_mut(next).previous = previous;
            } else {
                self.node_mut(parent).tail = previous;
            }
            if self.node_mut(parent).revoke_next == Some(id) {
                self.node_mut(parent).revoke_next = next;
            }
        }
        let node = self.node_mut(id);
        node.finished = true;
        node.previous = None;
        node.next = None;
        Ok(parent)
    }

    /// 已完成节点不再是父的活责任；观察壳仍可保留它，不阻止父完成。
    pub fn remove_finished(&mut self, id: NodeId) -> Result<RetiredNode<V, C>, Error> {
        if !self.node(id)?.finished {
            return Err(Error::Incomplete);
        }
        Ok(RetiredNode(
            self.nodes.remove(id.0).expect("finished node disappeared"),
        ))
    }
}
impl<V, C> Drop for ResourceTree<V, C> {
    fn drop(&mut self) {
        assert!(
            self.nodes.is_empty(),
            "resource tree dropped without bounded node retirement"
        );
    }
}
