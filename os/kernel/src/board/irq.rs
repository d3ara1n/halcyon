//! 首批 virt PLIC 准入；物理寄存器只留在内核，PCI 只取得经路由核对的 source 根。
use super::{BoardMemory, Cpu, MemoryRegion};
use dtb::{Fdt, NodeStatus, node_status, plic::Plic};
const CONTEXTS: usize = crate::hart::HART_NUM_LIMIT * 2;
pub(crate) const SOURCES: usize = 95;
#[derive(Clone, Copy)]
pub(crate) struct Platform {
    pub(crate) base: usize,
    contexts: [(usize, u32); CONTEXTS],
    len: usize,
    pub(crate) pci_node: u32,
    pub(crate) first: u32,
    pub(crate) count: u32,
}
impl Platform {
    pub(crate) fn context(&self, hartid: usize) -> Option<u32> {
        self.contexts[..self.len]
            .iter()
            .find(|(hart, _)| *hart == hartid)
            .map(|(_, context)| *context)
    }
}
pub(super) fn admit(
    fdt: &Fdt<'_>,
    memory: &BoardMemory,
    cpus: &[Cpu],
    pci_node: u32,
) -> Result<Platform, &'static str> {
    let soc = fdt.root().child("soc").ok_or("missing virt soc")?;
    let mut selected = None;
    for node in soc.children() {
        if !node
            .prop_str_list("compatible")
            .is_some_and(|mut list| list.any(|v| v == "riscv,plic0" || v == "sifive,plic-1.0.0"))
        {
            continue;
        }
        if node_status(&node).map_err(|_| "invalid PLIC status")? != NodeStatus::Okay {
            continue;
        }
        if selected.is_some() {
            return Err("multiple PLIC controllers are unsupported");
        }
        selected = Some(node);
    }
    let node = selected.ok_or("PCI host has no admitted PLIC")?;
    let plic = Plic::parse(fdt, &node).map_err(|_| "invalid PLIC description")?;
    if plic.base != 0x0c00_0000 || plic.bytes != 0x60_0000 || plic.sources != SOURCES as u32 {
        return Err("PLIC does not match the admitted virt machine");
    }
    let region = MemoryRegion {
        start: plic.base as usize,
        len: plic.bytes as usize,
    };
    if memory.memories[..memory.memory_len]
        .iter()
        .chain(&memory.platform_reservations[..memory.platform_reservation_len])
        .any(|excluded| region.overlaps(*excluded))
        || !super::contains_range(
            &memory.direct_map_regions[..memory.direct_map_region_len],
            region,
        )
    {
        return Err("PLIC registers overlap excluded or unmapped memory");
    }
    let pci = fdt
        .node_at_offset(pci_node as usize)
        .ok_or("invalid PCI node")?;
    let (first, count) = plic
        .virt_pci_sources(&pci)
        .map_err(|_| "unsupported PCI interrupt routing")?;
    let mut output = Platform {
        base: region.start,
        contexts: [(0, 0); CONTEXTS],
        len: 0,
        pci_node,
        first,
        count,
    };
    let mut all = [(u64::MAX, u32::MAX); CONTEXTS];
    for (context_count, item) in plic.contexts().enumerate() {
        let context = item.map_err(|_| "invalid PLIC context")?;
        if context_count == CONTEXTS
            || all[..context_count].contains(&(context.hartid, context.interrupt))
            || 0x20_0008u64 + context.index as u64 * 0x1000 > plic.bytes
        {
            return Err("duplicate or out-of-range PLIC context");
        }
        all[context_count] = (context.hartid, context.interrupt);
        if context.interrupt == 9 {
            output.contexts[output.len] = (
                usize::try_from(context.hartid).map_err(|_| "PLIC hartid overflow")?,
                context.index,
            );
            output.len += 1;
        }
    }
    for cpu in cpus.iter().filter(|cpu| cpu.mmu != super::MmuType::Bare) {
        if output.context(cpu.hartid).is_none() {
            return Err("admitted hart has no supervisor PLIC context");
        }
    }
    Ok(output)
}
