//! 依据资源树合同验证祖先封口、提交序号、单次父回执和门外退款。
use funded_storage::Funding;
use resource_tree::{Admission, Error, NodeId, PreparedNode, Progress, ResourceTree};
use std::{alloc::Layout, cell::Cell, rc::Rc};

#[derive(Clone, Default)]
struct Payer {
    live: Rc<Cell<usize>>,
    gate: Rc<Cell<bool>>,
}
struct Charge(Payer);
impl Funding for Payer {
    type Charge = Charge;
    type Error = ();
    fn charge(&self, _: Layout) -> Result<Charge, ()> {
        assert!(!self.gate.get(), "allocation inside resource gate");
        self.live.set(self.live.get() + 1);
        Ok(Charge(self.clone()))
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        assert!(!self.0.gate.get(), "refund inside resource gate");
        self.0.live.set(self.0.live.get() - 1);
    }
}
type Tree = ResourceTree<(), Charge>;
fn prepared(payer: &Payer) -> PreparedNode<(), Charge> {
    PreparedNode::try_new((), payer).unwrap_or_else(|_| panic!("node funding failed"))
}
fn checked(tree: &Tree, id: NodeId) -> Admission {
    let mut admission = tree.begin(id).unwrap();
    while tree.advance(&mut admission).unwrap() == Progress::Pending {}
    admission
}
fn child(tree: &mut Tree, parent: NodeId, payer: &Payer) -> NodeId {
    tree.link_child(&checked(tree, parent), prepared(payer))
        .unwrap_or_else(|_| panic!("child admission failed"))
}
fn retire(tree: &mut Tree, id: NodeId) {
    tree.seal(id).unwrap();
    tree.finish(id).unwrap();
    drop(tree.remove_finished(id).unwrap());
}

#[test]
fn closing_an_unvisited_ancestor_blocks_descendant_commit() {
    let payer = Payer::default();
    let mut tree = Tree::new(prepared(&payer));
    let root = tree.root();
    let branch = child(&mut tree, root, &payer);
    let leaf = child(&mut tree, branch, &payer);
    let mut in_flight = tree.begin(leaf).unwrap();
    assert_eq!(tree.advance(&mut in_flight), Ok(Progress::Pending));
    tree.revoke(root).unwrap();
    assert_eq!(tree.advance(&mut in_flight), Err(Error::Busy));
    // 新一轮尚能读取叶节点，但最终必须遇到已封祖先，后台尚未遍历不是漏洞。
    let mut later = tree.begin(leaf).unwrap();
    assert_eq!(tree.advance(&mut later), Ok(Progress::Pending));
    assert_eq!(tree.advance(&mut later), Ok(Progress::Pending));
    assert_eq!(tree.advance(&mut later), Err(Error::Closed));
    assert_eq!(tree.add_member(&later), Err(Error::Incomplete));
    retire(&mut tree, leaf);
    retire(&mut tree, branch);
    retire(&mut tree, root);
    assert_eq!(payer.live.get(), 0);
}

#[test]
fn seal_preserves_existing_members_but_invalidates_unrelated_preparations() {
    let payer = Payer::default();
    let mut tree = Tree::new(prepared(&payer));
    let root = tree.root();
    let left = child(&mut tree, root, &payer);
    let right = child(&mut tree, root, &payer);
    tree.add_member(&checked(&tree, left)).unwrap();
    let right_admission = checked(&tree, right);
    tree.seal(left).unwrap();
    assert_eq!(tree.validate(&right_admission), Err(Error::Busy));
    assert_eq!(tree.status(left).unwrap().members, 1);
    assert!(!tree.status(left).unwrap().revoking);
    assert_eq!(tree.finish(left), Err(Error::Incomplete));
    // 未封兄弟可在新的明确准入中成功；旧票不因兄弟仍开放而恢复。
    let current = checked(&tree, right);
    tree.add_member(&current).unwrap();
    tree.finish_member(left);
    tree.finish_member(right);
    retire(&mut tree, left);
    retire(&mut tree, right);
    retire(&mut tree, root);
}

#[test]
fn child_completion_repairs_cursor_and_returns_parent_responsibility_once() {
    let payer = Payer::default();
    let mut tree = Tree::new(prepared(&payer));
    let root = tree.root();
    let a = child(&mut tree, root, &payer);
    let b = child(&mut tree, root, &payer);
    let c = child(&mut tree, root, &payer);
    tree.revoke(root).unwrap();
    tree.revoke(a).unwrap();
    assert_eq!(tree.finish(a), Ok(Some(root)));
    assert_eq!(tree.finish(a), Ok(None));
    assert_eq!(tree.next_child_to_revoke(root), Ok(Some(b)));
    tree.revoke(c).unwrap();
    assert_eq!(tree.finish(c), Ok(Some(root)));
    assert_eq!(tree.next_child_to_revoke(root), Ok(None));
    // 遍历结束并不代表子责任完成。
    assert_eq!(tree.finish(root), Err(Error::Incomplete));
    tree.revoke(b).unwrap();
    assert_eq!(tree.finish(b), Ok(Some(root)));
    assert_eq!(tree.finish(root), Ok(None));
    // 完成子壳保留观察寿命不阻止父完成和节点存储退休。
    drop(tree.remove_finished(root).unwrap());
    assert_eq!(tree.begin(a).err(), Some(Error::Closed));
    for id in [a, b, c] {
        drop(tree.remove_finished(id).unwrap());
    }
    assert!(tree.is_empty());
}

#[test]
fn deep_ancestry_is_incremental_and_node_storage_is_retired_individually() {
    let payer = Payer::default();
    let mut tree = Tree::new(prepared(&payer));
    let mut ids = vec![tree.root()];
    for _ in 0..256 {
        ids.push(child(&mut tree, *ids.last().unwrap(), &payer));
    }
    let mut admission = tree.begin(*ids.last().unwrap()).unwrap();
    for _ in 1..ids.len() {
        assert_eq!(tree.advance(&mut admission), Ok(Progress::Pending));
        assert_eq!(tree.validate(&admission), Err(Error::Incomplete));
    }
    assert_eq!(tree.advance(&mut admission), Ok(Progress::Checked));
    tree.add_member(&admission).unwrap();
    tree.finish_member(*ids.last().unwrap());
    for id in ids.into_iter().rev() {
        retire(&mut tree, id);
    }
    assert_eq!(payer.live.get(), 0);
}

#[test]
fn tickets_cannot_authorize_another_root_and_rejected_nodes_refund_outside_gate() {
    let payer = Payer::default();
    let mut a = Tree::new(prepared(&payer));
    let mut b = Tree::new(prepared(&payer));
    let ar = a.root();
    let br = b.root();
    let admission = checked(&a, ar);
    let candidate = prepared(&payer);
    payer.gate.set(true);
    assert_eq!(b.validate(&admission), Err(Error::WrongRoot));
    let (error, candidate) = match b.link_child(&admission, candidate) {
        Err(rejected) => rejected,
        Ok(_) => panic!("foreign ticket linked a child"),
    };
    assert_eq!(error, Error::WrongRoot);
    a.seal(ar).unwrap();
    a.finish(ar).unwrap();
    let retired = a.remove_finished(ar).unwrap();
    assert_eq!(payer.live.get(), 3);
    payer.gate.set(false);
    drop(candidate);
    drop(retired);
    retire(&mut b, br);
    assert_eq!(payer.live.get(), 0);
}
