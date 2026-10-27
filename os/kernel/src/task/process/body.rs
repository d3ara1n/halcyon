//! 进程私有资源体：只能经 Shell 的短租约借用，不能由外部强持。

use alloc::sync::Arc;

use erhino_shared::call::SystemCallError;

use crate::{
    sync::{Spinlock, ranks},
    vm::{change::VmShell, retire::RetireWork, space::AnySpace, views::Views},
};

use super::{
    super::{handle::ProcessCapTable, memory_pool::MemoryPool},
    members::Members,
};

pub(crate) type BodyStorage =
    funded_storage::OwnedBox<ProcessBody, super::super::resources::MetadataCharge>;

pub(crate) struct ProcessBody {
    pub(crate) vm: Arc<VmShell>,
    pub(crate) retire: Option<Arc<RetireWork>>,
    pub(crate) views: Arc<Views>,
    pub(crate) handles: Spinlock<ProcessCapTable>,
    pub(crate) members: Members,
    pub(crate) execution: Spinlock<Option<&'static crate::sched::SchedDomain>>,
    pub(crate) requirement: Spinlock<Option<elf::IsaRequirement>>,
}

impl ProcessBody {
    pub(crate) fn try_new(
        space: AnySpace,
        pool: &Arc<MemoryPool>,
        handles: ProcessCapTable,
        source: Arc<super::super::wait_source::SignalSource>,
    ) -> Result<Self, SystemCallError> {
        let views = space.views().clone();
        let vm = VmShell::try_new(space, pool)?;
        let retire = RetireWork::new(pool.clone(), vm.clone(), views.clone(), source)?;
        Ok(Self {
            vm,
            retire: Some(retire),
            views,
            handles: Spinlock::new(ranks::HANDLE_TABLE, handles),
            members: Members::new(),
            execution: Spinlock::new(ranks::LIFECYCLE, None),
            requirement: Spinlock::new(ranks::LEAF, None),
        })
    }
}
