//! FAL 领域资源分类：额度机制由 libsrv::budget 提供，分类含义归领域。

use libsrv::budget::Taxonomy;

/// FAL 的资源分类；将来接入执行核心时由 FAL 服务自行追加任务/输入槽位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalResource {
    Node,
    Bytes,
    Grant,
    Request,
    Outbox,
    Watch,
    Offer,
    Stream,
    WaitSource,
    ServiceRecord,
}

impl Taxonomy for FalResource {
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
            Self::ServiceRecord => 9,
        }
    }
}
