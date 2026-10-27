//! 对外不可设置的序号耗尽边界；耗尽只封准入，不阻止既有责任退休。
use super::*;
struct Payer;
impl Funding for Payer {
    type Charge = ();
    type Error = ();
    fn charge(&self, _: core::alloc::Layout) -> Result<(), ()> {
        Ok(())
    }
}
fn node() -> PreparedNode<(), ()> {
    PreparedNode::try_new((), &Payer).unwrap_or_else(|_| panic!("allocation failed"))
}
#[test]
fn exhausted_close_sequence_permanently_seals_admission_but_allows_retirement() {
    let mut tree = ResourceTree::new(node());
    let root = tree.root();
    let mut admission = tree.begin(root).unwrap();
    assert_eq!(tree.advance(&mut admission), Ok(Progress::Checked));
    let child = tree
        .link_child(&admission, node())
        .unwrap_or_else(|_| panic!("link failed"));
    tree.add_member(&admission).unwrap();
    tree.sequence = Some(u64::MAX);
    let mut pending = tree.begin(root).unwrap();
    assert_eq!(tree.advance(&mut pending), Ok(Progress::Checked));
    tree.seal(child).unwrap();
    assert_eq!(tree.validate(&pending), Err(Error::Closed));
    assert_eq!(tree.begin(root).err(), Some(Error::Closed));
    tree.revoke(root).unwrap();
    tree.finish_member(root);
    tree.finish(child).unwrap();
    drop(tree.remove_finished(child).unwrap());
    tree.finish(root).unwrap();
    drop(tree.remove_finished(root).unwrap());
    assert!(tree.is_empty());
}
