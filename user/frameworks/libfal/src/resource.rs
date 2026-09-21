//! FAL 领域资源分类：额度机制由 libsrv::budget 提供，分类含义归领域。

use libsrv::budget::{ExecutionSlots, Taxonomy};

/// FAL 的资源分类；将来接入执行核心时由 FAL 服务自行追加任务/输入槽位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FalResource {
    Task,
    InputBytes,
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
    const COUNT: usize = 12;
    fn slot(self) -> usize {
        match self {
            Self::Task => 0,
            Self::InputBytes => 1,
            Self::Node => 2,
            Self::Bytes => 3,
            Self::Grant => 4,
            Self::Request => 5,
            Self::Outbox => 6,
            Self::Watch => 7,
            Self::Offer => 8,
            Self::Stream => 9,
            Self::WaitSource => 10,
            Self::ServiceRecord => 11,
        }
    }
}

impl FalResource {
    pub const EXECUTION_SLOTS: ExecutionSlots = ExecutionSlots {
        task: 0,
        input_bytes: 1,
    };
}
