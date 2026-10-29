//! init 的平台输入：内核交付的只读平台描述与按记录序的资源根。
//!
//! init 只核描述头与初始槽关联，不解释 DTB 或资源几何；这些由接收平台根的实例核验。
use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    object::{Handle, Rights},
    platform::{Description, ResourceKind},
};
use libkernel::{
    ipc::capability::Capability,
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
};
use libprocess::context::InitialGrants;

/// 路由平台描述时的权利；接收方按此核验只读描述对象。
pub(crate) const DESCRIPTION_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
/// 路由平台根时的权利。
pub(crate) const ROOT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

pub(crate) struct Root {
    pub kind: ResourceKind,
    pub capability: Capability,
}

pub(crate) struct Platform {
    description: Capability,
    roots: Vec<Root>,
}

impl Platform {
    /// 核描述头与初始槽数，并按记录序取出平台根。
    pub(crate) fn adopt(
        description: Capability,
        grants: &mut InitialGrants,
    ) -> Result<Self, SystemCallError> {
        // SAFETY: 只借用 handle 建立只读映射；关闭责任仍由 `description` 持有。
        let object = core::mem::ManuallyDrop::new(unsafe {
            MemoryObject::from_handle(description.as_handle())
        });
        let length =
            usize::try_from(object.query()?.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
        let mapping = MappedRegion::map_object(
            &object,
            0,
            length,
            0,
            0,
            MemoryProtection::ReadOnly,
            Placement::Anywhere,
        )?;
        let start = mapping
            .usable()
            .ok_or(SystemCallError::IllegalArgument)?
            .start;
        // SAFETY: 映射覆盖 length 字节且在本函数内保持有效。
        let bytes = unsafe { core::slice::from_raw_parts(start as *const u8, length) };
        let roots = (|| {
            let facts = Description::parse(bytes, grants.len())
                .map_err(|_| SystemCallError::IllegalArgument)?;
            let count = facts.header().resource_count;
            let mut roots = Vec::new();
            roots
                .try_reserve_exact(count)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            for index in 0..count {
                let resource = facts
                    .resource(index)
                    .ok_or(SystemCallError::IllegalArgument)?;
                roots.push(Root {
                    kind: resource.kind,
                    capability: grants
                        .take(resource.handle_index as usize)
                        .ok_or(SystemCallError::IllegalArgument)?,
                });
            }
            Ok(roots)
        })();
        mapping.unmap().map_err(|(_, error)| error)?;
        Ok(Self {
            description,
            roots: roots?,
        })
    }

    pub(crate) fn description(&self) -> Handle {
        self.description.as_handle()
    }

    pub(crate) fn roots(&self) -> &[Root] {
        &self.roots
    }

    /// 终局封全部平台根，按实际种类调用对应 seal；失败只记录，不中断终局。
    pub(crate) fn seal_all(&self) {
        for root in &self.roots {
            let handle = root.capability.as_handle();
            // SAFETY: 种类来自已核描述记录，只借用 handle 发起 seal，不转移关闭责任。
            let result = unsafe {
                match root.kind {
                    ResourceKind::MmioControl => core::mem::ManuallyDrop::new(
                        libkernel::mmio::MmioControl::from_handle(handle),
                    )
                    .seal(),
                    ResourceKind::IrqControl => core::mem::ManuallyDrop::new(
                        libkernel::irq::IrqControl::from_handle(handle),
                    )
                    .seal(),
                    ResourceKind::DmaDomainControl => core::mem::ManuallyDrop::new(
                        libkernel::dma::DmaDomainControl::from_handle(handle),
                    )
                    .seal(),
                }
            };
            match result {
                Ok(()) => libkernel::debug!("system failure platform root Seal committed"),
                Err(error) => {
                    libkernel::debug!("system failure resource Seal unconfirmed: {:?}", error)
                }
            }
        }
    }
}
