//! 只读平台输入与 PCI host 的实际控制能力核验；几何只在这里解释一次。
use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    mem::MemoryProtection,
    object::{Handle, ObjectKind, Rights},
    platform::{Description, Header, ResourceKind},
};
use libkernel::{
    dma::DmaDomainControl,
    ipc::{capability::Capability, object},
    irq::IrqControl,
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
    mmio::MmioControl,
};

pub const ROOT_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::CREATE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub const DESCRIPTION_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

pub struct DescriptionObject {
    object: MemoryObject,
    mapping: Option<MappedRegion>,
    header: Header,
}
impl DescriptionObject {
    pub fn adopt(cap: Capability) -> Result<Self, SystemCallError> {
        let info = cap.description()?;
        if info.kind != ObjectKind::MemoryObject as u32 || info.rights != DESCRIPTION_RIGHTS {
            return Err(SystemCallError::RightsDenied);
        }
        // SAFETY: 已核实际类型，唯一关闭责任移入。
        let object = unsafe { MemoryObject::from_handle(cap.into_raw()) };
        let length =
            usize::try_from(object.query()?.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
        if length < erhino_shared::platform::HEADER_BYTES || length > isize::MAX as usize {
            return Err(SystemCallError::IllegalArgument);
        }
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
            .expect("platform mapping has no usable range")
            .start;
        // SAFETY: 启动合同交付已初始化、无写 capability/DMA 写者的只读对象；映射覆盖 length。
        let bytes = unsafe { core::slice::from_raw_parts(start as *const u8, length) };
        let result = (|| {
            let description =
                Description::decode(bytes).map_err(|_| SystemCallError::IllegalArgument)?;
            if bytes[description.header().total_bytes..]
                .iter()
                .any(|byte| *byte != 0)
            {
                return Err(SystemCallError::IllegalArgument);
            }
            let tree =
                dtb::Fdt::new(description.dtb()).map_err(|_| SystemCallError::IllegalArgument)?;
            if tree.total_size() != description.header().dtb_bytes {
                return Err(SystemCallError::IllegalArgument);
            }
            Ok(description.header())
        })();
        match result {
            Ok(header) => Ok(Self {
                object,
                mapping: Some(mapping),
                header,
            }),
            Err(error) => {
                let _ = mapping.unmap();
                Err(error)
            }
        }
    }
    pub fn handle(&self) -> Handle {
        self.object.handle()
    }
    pub fn bytes(&self) -> &[u8] {
        let start = self
            .mapping
            .as_ref()
            .expect("platform mapping missing")
            .usable()
            .expect("platform mapping lost usable range")
            .start;
        // SAFETY: 私有只读映射跟随本值保活，无可写别名；借用不越过映射寿命。
        unsafe { core::slice::from_raw_parts(start as *const u8, self.header.total_bytes) }
    }
    pub fn description(&self) -> Description<'_> {
        Description::decode(self.bytes()).expect("validated platform description changed")
    }
}
impl Drop for DescriptionObject {
    fn drop(&mut self) {
        // 无 MMIO 来源或并发修改者；失败时实物 View 仍由进程 VM 退出路径退休。
        if let Some(mapping) = self.mapping.take() {
            let _ = mapping.unmap();
        }
    }
}

pub enum Control {
    Mmio { control: MmioControl, ecam: bool },
    Irq(IrqControl),
    Dma(DmaDomainControl),
}
impl Control {
    pub fn handle(&self) -> Handle {
        match self {
            Self::Mmio { control, .. } => control.handle(),
            Self::Irq(control) => control.handle(),
            Self::Dma(control) => control.handle(),
        }
    }
}
/// controls 顺序与只读描述的记录序号一致；从不使用原内核 handle_index 寻址子进程。
pub struct Platform {
    description: DescriptionObject,
    controls: Vec<Control>,
}
impl Platform {
    /// 从实际委托中选择首批 non-prefetchable memory window；索引只定位本值持有的控制根。
    pub fn host(&self) -> Result<Option<(crate::host::HostInfo, [usize; 4])>, SystemCallError> {
        if self.controls.is_empty() {
            return Ok(None);
        }
        let facts = self.description.description();
        let tree = dtb::Fdt::new(facts.dtb()).map_err(|_| SystemCallError::IllegalArgument)?;
        let node = tree
            .node_at_offset(
                facts
                    .resource(0)
                    .ok_or(SystemCallError::IllegalArgument)?
                    .dt_node_offset as usize,
            )
            .ok_or(SystemCallError::IllegalArgument)?;
        let host =
            dtb::pci::Host::parse(&node, 2, 2).map_err(|_| SystemCallError::IllegalArgument)?;
        let window = host
            .windows()
            .find_map(|window| {
                window.ok().filter(|window| {
                    window.space == dtb::pci::Space::Memory32 && !window.prefetchable
                })
            })
            .ok_or(SystemCallError::IllegalArgument)?;
        let mut indices = [usize::MAX; 4];
        let mut targets = 0;
        for (index, control) in self.controls.iter().enumerate() {
            match control {
                Control::Mmio { ecam: true, .. } => indices[0] = index,
                Control::Mmio {
                    control,
                    ecam: false,
                } if control.query()?.physical == window.cpu.start => indices[1] = index,
                Control::Irq(control) => {
                    indices[2] = index;
                    targets = control.query()?.scope.targets;
                }
                Control::Dma(_) => indices[3] = index,
                _ => {}
            }
        }
        if indices.contains(&usize::MAX) {
            return Err(SystemCallError::IllegalArgument);
        }
        // adopt 已经用 PLIC parser 核对完整固定形状；这里保留实际 map 的逐项路由事实。
        let map = node
            .prop("interrupt-map")
            .ok_or(SystemCallError::IllegalArgument)?;
        let mut intx = [0; 16];
        for entry in map.as_chunks::<24>().0 {
            let word = |at| {
                u32::from_be_bytes(
                    entry[at..at + 4]
                        .try_into()
                        .expect("validated interrupt map width changed"),
                )
            };
            intx[((word(0) >> 11) * 4 + word(12) - 1) as usize] = word(20);
        }
        Ok(Some((
            crate::host::HostInfo {
                first_bus: host.first_bus,
                last_bus: host.last_bus,
                ecam_cpu: host.ecam.start,
                ecam_bytes: host.ecam.len(),
                window_bus: window.bus.start,
                window_cpu: window.cpu.start,
                window_bytes: window.cpu.len(),
                intx,
                targets,
            },
            indices,
        )))
    }
    pub fn adopt(
        description: DescriptionObject,
        caps: Vec<Capability>,
    ) -> Result<Self, SystemCallError> {
        let facts = description.description();
        if caps.len() != facts.header().resource_count {
            return Err(SystemCallError::IllegalArgument);
        }
        let tree = dtb::Fdt::new(facts.dtb()).map_err(|_| SystemCallError::IllegalArgument)?;
        let mut controls = Vec::new();
        controls
            .try_reserve_exact(caps.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let mut host_offset = None;
        for (index, cap) in caps.into_iter().enumerate() {
            let resource = facts
                .resource(index)
                .ok_or(SystemCallError::IllegalArgument)?;
            if host_offset.is_some_and(|offset| offset != resource.dt_node_offset) {
                return Err(SystemCallError::IllegalArgument);
            }
            host_offset = Some(resource.dt_node_offset);
            let node = tree
                .node_at_offset(resource.dt_node_offset as usize)
                .ok_or(SystemCallError::IllegalArgument)?;
            if !node
                .prop_str_list("compatible")
                .is_some_and(|mut values| values.any(|value| value == "pci-host-ecam-generic"))
            {
                return Err(SystemCallError::IllegalArgument);
            }
            let host =
                dtb::pci::Host::parse(&node, 2, 2).map_err(|_| SystemCallError::IllegalArgument)?;
            let info = object::query(cap.as_handle())?;
            let expected = match resource.kind {
                ResourceKind::MmioControl => ObjectKind::MmioControl,
                ResourceKind::IrqControl => ObjectKind::IrqControl,
                ResourceKind::DmaDomainControl => ObjectKind::DmaDomainControl,
            };
            if info.kind != expected as u32 || info.rights != ROOT_RIGHTS {
                return Err(SystemCallError::RightsDenied);
            }
            let control = match resource.kind {
                ResourceKind::MmioControl => {
                    // SAFETY: 上面已核实际角色，唯一关闭责任移入。
                    let control = unsafe { MmioControl::from_handle(cap.into_raw()) };
                    let geometry = control.query()?;
                    let ecam =
                        geometry.physical == host.ecam.start && geometry.bytes == host.ecam.len();
                    if geometry.flags != 0
                        || geometry.members != 0
                        || geometry.writable != 1
                        || geometry.reserved != [0; 2]
                        || (!ecam
                            && !host.windows().any(|window| {
                                window.is_ok_and(|window| {
                                    window.space != dtb::pci::Space::Io
                                        && window.cpu.start == geometry.physical
                                        && window.cpu.len() == geometry.bytes
                                })
                            }))
                    {
                        return Err(SystemCallError::IllegalArgument);
                    }
                    for previous in &controls {
                        if let Control::Mmio {
                            control: previous, ..
                        } = previous
                            && previous.query()?.physical == geometry.physical
                        {
                            return Err(SystemCallError::IllegalArgument);
                        }
                    }
                    Control::Mmio { control, ecam }
                }
                ResourceKind::IrqControl => {
                    if controls
                        .iter()
                        .any(|previous| matches!(previous, Control::Irq(_)))
                    {
                        return Err(SystemCallError::IllegalArgument);
                    }
                    // SAFETY: 实际 kind/rights 已核定。
                    let control = unsafe { IrqControl::from_handle(cap.into_raw()) };
                    let snapshot = control.query()?;
                    let soc = tree
                        .root()
                        .child("soc")
                        .ok_or(SystemCallError::IllegalArgument)?;
                    let plic_node = soc
                        .children()
                        .find(|node| {
                            node.prop_str_list("compatible").is_some_and(|mut values| {
                                values.any(|value| value == "riscv,plic0")
                            })
                        })
                        .ok_or(SystemCallError::IllegalArgument)?;
                    let plic = dtb::plic::Plic::parse(&tree, &plic_node)
                        .map_err(|_| SystemCallError::IllegalArgument)?;
                    let (first, count) = plic
                        .virt_pci_sources(&node)
                        .map_err(|_| SystemCallError::IllegalArgument)?;
                    if snapshot.flags != 0
                        || snapshot.generation != 0
                        || snapshot.scope.first != first
                        || snapshot.scope.count != count
                        || snapshot.scope.targets == 0
                    {
                        return Err(SystemCallError::IllegalArgument);
                    }
                    Control::Irq(control)
                }
                ResourceKind::DmaDomainControl => {
                    if controls
                        .iter()
                        .any(|previous| matches!(previous, Control::Dma(_)))
                    {
                        return Err(SystemCallError::IllegalArgument);
                    }
                    // SAFETY: 实际 kind/rights 已核定。
                    let control = unsafe { DmaDomainControl::from_handle(cap.into_raw()) };
                    let snapshot = control.query()?;
                    if !host.coherent
                        || snapshot.mode != erhino_shared::dma::TRUSTED_PHYSICAL
                        || snapshot.coherency != erhino_shared::dma::COHERENT
                        || snapshot.flags != 0
                        || snapshot.members != 0
                        || snapshot.reserved != 0
                    {
                        return Err(SystemCallError::IllegalArgument);
                    }
                    Control::Dma(control)
                }
            };
            controls.push(control);
        }
        // 首批 PCI host 必须有实际 ECAM、非预取 memory window、IRQ 和 coherent DMA；零根平台不造设备。
        if !controls.is_empty()
            && (!controls
                .iter()
                .any(|c| matches!(c, Control::Mmio { ecam: true, .. }))
                || !controls
                    .iter()
                    .any(|c| matches!(c, Control::Mmio { ecam: false, .. }))
                || !controls.iter().any(|c| matches!(c, Control::Irq(_)))
                || !controls.iter().any(|c| matches!(c, Control::Dma(_))))
        {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            description,
            controls,
        })
    }
    pub fn description_object(&self) -> &DescriptionObject {
        &self.description
    }
    pub fn controls(&self) -> &[Control] {
        &self.controls
    }
    pub fn ecam_handle(&self) -> Option<Handle> {
        self.controls.iter().find_map(|c| match c {
            Control::Mmio {
                control,
                ecam: true,
            } => Some(control.handle()),
            _ => None,
        })
    }
    pub fn irq_handle(&self) -> Option<Handle> {
        self.controls.iter().find_map(|c| match c {
            Control::Irq(control) => Some(control.handle()),
            _ => None,
        })
    }
    pub fn dma_handle(&self) -> Option<Handle> {
        self.controls.iter().find_map(|c| match c {
            Control::Dma(control) => Some(control.handle()),
            _ => None,
        })
    }
}
