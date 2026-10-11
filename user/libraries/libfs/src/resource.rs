//! 文件领域资源分类；付款账户与领域视图由 `libbudget` 提供。

use libbudget::Taxonomy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsResource {
    Node,
    Bytes,
    Grant,
    Request,
    Outbox,
    Watch,
    Offer,
    Stream,
    WaitSource,
    Binding,
}

impl Taxonomy for FsResource {
    const COUNT: usize = 10;

    fn slot(self) -> usize {
        match self {
            Self::Node => 0,
            Self::Bytes => 1,
            Self::Grant => 2,
            Self::Request => 3,
            Self::Outbox => 4,
            Self::Watch => 5,
            Self::Offer => 6,
            Self::Stream => 7,
            Self::WaitSource => 8,
            Self::Binding => 9,
        }
    }
}
