//! init 保留平台根 owner；内核初始槽校验与用户态服务委托分别验证。
use alloc::vec::Vec;
use erhino_shared::{call::SystemCallError, object::Handle, platform::Description};
use libkernel::ipc::capability::Capability;
use libpci::platform::{DescriptionObject, Platform};
use libprocess::context::InitialGrants;

pub(super) struct PlatformDescription {
    platform: Platform,
}
impl PlatformDescription {
    pub(super) fn adopt(
        capability: Capability,
        grants: &mut InitialGrants,
    ) -> Result<Self, SystemCallError> {
        let description = DescriptionObject::adopt(capability)?;
        // kernel→init 的实际数组长度与每条初始槽保持严格核验；后继委托不复用它。
        let facts = Description::parse(description.bytes(), grants.len())
            .map_err(|_| SystemCallError::IllegalArgument)?;
        let mut caps = Vec::new();
        caps.try_reserve_exact(facts.header().resource_count)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        for index in 0..facts.header().resource_count {
            let resource = facts
                .resource(index)
                .ok_or(SystemCallError::IllegalArgument)?;
            caps.push(
                grants
                    .take(resource.handle_index as usize)
                    .ok_or(SystemCallError::IllegalArgument)?,
            );
        }
        Ok(Self {
            platform: Platform::adopt(description, caps)?,
        })
    }
    pub(super) fn ecam_handle(&self) -> Option<Handle> {
        self.platform.ecam_handle()
    }
    pub(super) fn irq_handle(&self) -> Option<Handle> {
        self.platform.irq_handle()
    }
    pub(super) fn dma_handle(&self) -> Option<Handle> {
        self.platform.dma_handle()
    }
    pub(super) fn description(&self) -> Description<'_> {
        self.platform.description_object().description()
    }
    pub(super) fn platform(&self) -> &Platform {
        &self.platform
    }
}
