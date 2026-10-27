//! QEMU virt 的物理 IO admission。数值核验来自固定 virt.c 机器布局，
//! DT 几何仍由共享 parser 解释；本模块是唯一把平台事实变成可委托区间的位置。
use super::{BoardMemory, MemoryRegion};
use dtb::{
    Fdt, Node, NodeStatus, node_status,
    pci::{Host, Space},
};

// 当前 virt host 只有 ECAM、32 位 memory 和 64 位 memory 三个可委托窗口。
pub(crate) const MAX_MMIO_ROOTS: usize = 3;
#[derive(Clone, Copy)]
pub(crate) struct MmioRoot {
    pub(crate) node_offset: u32,
    pub(crate) start: usize,
    pub(crate) bytes: usize,
}
#[derive(Clone, Copy)]
pub(crate) struct IoRoots {
    pub(crate) dma: Option<DmaRoot>,
    pub(crate) irq: Option<super::irq::Platform>,
    mmio: [MmioRoot; MAX_MMIO_ROOTS],
    len: usize,
}
impl IoRoots {
    pub(crate) const fn empty() -> Self {
        Self {
            dma: None,
            irq: None,
            mmio: [MmioRoot {
                node_offset: 0,
                start: 0,
                bytes: 0,
            }; MAX_MMIO_ROOTS],
            len: 0,
        }
    }
    pub(crate) fn mmio(&self) -> &[MmioRoot] {
        &self.mmio[..self.len]
    }
    pub(crate) fn resource_count(&self) -> usize {
        self.len + usize::from(self.irq.is_some()) + usize::from(self.dma.is_some())
    }
}
#[derive(Clone, Copy)]
pub(crate) struct DmaRoot {
    pub(crate) node_offset: u32,
}
fn compatible(node: &Node<'_, '_>, name: &str) -> bool {
    node.prop_str_list("compatible")
        .is_some_and(|mut values| values.any(|value| value == name))
}
fn enabled(node: &Node<'_, '_>) -> Result<bool, &'static str> {
    node_status(node)
        .map(|status| status == NodeStatus::Okay)
        .map_err(|_| "invalid IO node status")
}
fn cells(node: &Node<'_, '_>, name: &str, value: u32) -> bool {
    node.prop(name) == Some(value.to_be_bytes().as_slice())
}
// 缺省 DMA 恒等来自已选 QEMU 机器实现，不从通用 DT 的属性缺省推导。
fn identity_dma(node: &Node<'_, '_>) -> bool {
    node.prop("dma-ranges").is_none_or(|value| value.is_empty())
        && ["iommus", "iommu-map", "memory-region", "dma-noncoherent"]
            .iter()
            .all(|property| node.prop(property).is_none())
}
pub(super) fn admit(fdt: &Fdt<'_>, memory: &BoardMemory) -> Result<IoRoots, &'static str> {
    let root = fdt.root();
    if !compatible(&root, "riscv-virtio") {
        return Ok(IoRoots::empty());
    }
    if !enabled(&root)? {
        return Ok(IoRoots::empty());
    }
    let Some(soc) = root.child("soc") else {
        return Ok(IoRoots::empty());
    };
    if !enabled(&soc)? {
        return Ok(IoRoots::empty());
    }
    if !cells(&root, "#address-cells", 2)
        || !cells(&root, "#size-cells", 2)
        || !cells(&soc, "#address-cells", 2)
        || !cells(&soc, "#size-cells", 2)
        || soc.prop("ranges") != Some(&[][..])
        || !compatible(&soc, "simple-bus")
        || !identity_dma(&root)
        || !identity_dma(&soc)
    {
        return Err("unsupported virt parent address translation");
    }
    let mut output = IoRoots::empty();
    for node in soc.children() {
        if !compatible(&node, "pci-host-ecam-generic") || !enabled(&node)? {
            continue;
        }
        if output.len != 0 {
            return Err("multiple PCI hosts are unsupported");
        }
        if !identity_dma(&node)
            || node.prop("msi-parent").is_some()
            || node.prop("msi-map").is_some()
        {
            return Err("unsupported PCI DMA or MSI topology");
        }
        let host = Host::parse(&node, 2, 2).map_err(|_| "invalid PCI ECAM geometry")?;
        if host.first_bus != 0
            || host.last_bus != 255
            || !host.coherent
            || host.ecam.start != 0x3000_0000
            || host.ecam.len() != 0x1000_0000
        {
            return Err("PCI ECAM does not match the admitted virt machine");
        }
        let offset = u32::try_from(host.node_offset).map_err(|_| "DT node offset overflow")?;
        output.mmio[0] = MmioRoot {
            node_offset: offset,
            start: host.ecam.start as usize,
            bytes: host.ecam.len() as usize,
        };
        let mut seen = [false; 3];
        for window in host.windows() {
            let window = window.map_err(|_| "invalid PCI range")?;
            let (index, bus, cpu, bytes) = match window.space {
                Space::Io => (0, 0, 0x0300_0000, 0x1_0000),
                Space::Memory32 => (1, 0x4000_0000, 0x4000_0000, 0x4000_0000),
                Space::Memory64 => (2, 0x4_0000_0000, 0x4_0000_0000, 0x4_0000_0000),
            };
            if seen[index]
                || window.prefetchable
                || window.bus.start != bus
                || window.cpu.start != cpu
                || window.cpu.len() != bytes
            {
                return Err("PCI range does not match the admitted virt machine");
            }
            seen[index] = true;
            if index != 0 {
                output.mmio[index] = MmioRoot {
                    node_offset: offset,
                    start: window.cpu.start as usize,
                    bytes: window.cpu.len() as usize,
                };
            }
        }
        if !seen[1] || !seen[2] {
            return Err("virt PCI memory windows are incomplete");
        }
        output.len = MAX_MMIO_ROOTS;
        // 只来自固定 QEMU 无 IOMMU 的 coherent PCI host；不是从一般 DT 缺省推导恒等。
        output.dma = Some(DmaRoot {
            node_offset: offset,
        });
        for (index, io) in output.mmio().iter().enumerate() {
            let region = MemoryRegion {
                start: io.start,
                len: io.bytes,
            };
            if !region.start.is_multiple_of(super::PAGE_SIZE)
                || !region.len.is_multiple_of(super::PAGE_SIZE)
                || region.end() / super::PAGE_SIZE > page_table::MAX_PPN
                || memory.memories[..memory.memory_len]
                    .iter()
                    .chain(&memory.platform_reservations[..memory.platform_reservation_len])
                    .any(|excluded| region.overlaps(*excluded))
                || output.mmio()[..index].iter().any(|other| {
                    region.overlaps(MemoryRegion {
                        start: other.start,
                        len: other.bytes,
                    })
                })
            {
                return Err("PCI IO overlaps excluded physical memory");
            }
        }
    }
    Ok(output)
}
