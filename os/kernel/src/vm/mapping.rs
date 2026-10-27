//! VM 实物来源的单一分发点；RAM 退款与 MMIO 片段回执各自归原 owner。

use super::backing::{Backing, BackingRelease, BackingSplit, BackingUse};
use crate::resource::mmio::{MmioMapping, MmioSplit, MmioViewUse};
use crate::task::memory_pool::MemoryPool;
use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;
use memory_space::MappingOrigin;

pub(crate) enum MappingUse {
    Ram(BackingUse),
    Mmio(MmioViewUse),
}
impl From<BackingUse> for MappingUse {
    fn from(value: BackingUse) -> Self {
        Self::Ram(value)
    }
}
impl From<MmioViewUse> for MappingUse {
    fn from(value: MmioViewUse) -> Self {
        Self::Mmio(value)
    }
}
impl MappingUse {
    pub(crate) fn range(&self) -> (u64, u64) {
        match self {
            Self::Ram(value) => value.range(),
            Self::Mmio(value) => value.range(),
        }
    }
    pub(crate) fn matches_origin(&self, origin: MappingOrigin) -> bool {
        match (self, origin) {
            (Self::Ram(value), MappingOrigin::Anonymous { identity, .. }) => {
                value.backing().id() == identity
            }
            (Self::Ram(_), MappingOrigin::Object { .. }) => true,
            (Self::Mmio(value), MappingOrigin::Mmio { mapping, .. }) => {
                value.mapping().id() == mapping
            }
            _ => false,
        }
    }
    pub(crate) fn physical_span(&self, page: u64) -> Option<(page_table::Ppn, usize)> {
        match self {
            Self::Ram(value) => value.physical_span(page),
            Self::Mmio(value) => value.physical_span(page),
        }
    }
    pub(crate) fn split_source(&self) -> SplitSource {
        match self {
            Self::Ram(value) => SplitSource::Ram(value.backing().clone()),
            Self::Mmio(value) => SplitSource::Mmio(value.mapping().clone()),
        }
    }
    pub(crate) fn split_prepared(&mut self, page: u64, prepared: &mut MappingSplit) -> Self {
        match (self, prepared) {
            (Self::Ram(value), MappingSplit::Ram(prepared)) => {
                Self::Ram(value.split_prepared(page, prepared))
            }
            (Self::Mmio(value), MappingSplit::Mmio(prepared)) => {
                Self::Mmio(value.split(page, prepared))
            }
            _ => panic!("mapping split changed resource kind"),
        }
    }
    pub(crate) fn retire(self) -> MappingRelease {
        match self {
            Self::Ram(value) => MappingRelease::Ram(value.retire()),
            Self::Mmio(value) => MappingRelease::Mmio(Some(value)),
        }
    }
}

/// 只保活来源，不新增业务 use。节点与 split 准备均在 Views 门外付款/准入。
pub(crate) enum SplitSource {
    Ram(Arc<Backing>),
    Mmio(Arc<MmioMapping>),
}
impl SplitSource {
    pub(crate) fn prepare(&self, pool: &Arc<MemoryPool>) -> Result<MappingSplit, SystemCallError> {
        match self {
            Self::Ram(value) => Ok(MappingSplit::Ram(value.prepare_split(pool)?)),
            Self::Mmio(value) => Ok(MappingSplit::Mmio(MmioSplit::prepare(value)?)),
        }
    }
}
pub(crate) enum MappingSplit {
    Ram(BackingSplit),
    Mmio(MmioSplit),
}
pub(crate) enum MappingRelease {
    Ram(BackingRelease),
    Mmio(Option<MmioViewUse>),
}
impl MappingRelease {
    pub(crate) fn step(&mut self) -> bool {
        match self {
            Self::Ram(value) => value.step(),
            Self::Mmio(value) => {
                value
                    .take()
                    .expect("MMIO retirement advanced twice")
                    .retire();
                true
            }
        }
    }
}
