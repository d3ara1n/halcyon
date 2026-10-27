//! PLIC 的 DT 几何与 interrupts-extended CPU 关系；不推导授权。
use crate::{Fdt, Node, NodeStatus, cells_u64, node_status, property_string};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Unsupported,
    Missing,
    Duplicate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub index: u32,
    pub hartid: u64,
    pub interrupt: u32,
}
pub struct Plic<'a, 'dt> {
    pub base: u64,
    pub bytes: u64,
    pub sources: u32,
    pub phandle: u32,
    extended: &'a [u8],
    tree: &'a Fdt<'dt>,
}
fn word(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::Malformed)?
            .try_into()
            .map_err(|_| Error::Malformed)?,
    ))
}
fn property(node: &Node<'_, '_>, name: &str) -> Result<u32, Error> {
    let bytes = node.prop(name).ok_or(Error::Missing)?;
    if bytes.len() != 4 {
        return Err(Error::Malformed);
    }
    word(bytes, 0)
}
impl<'a, 'dt: 'a> Plic<'a, 'dt> {
    pub fn parse(tree: &'a Fdt<'dt>, node: &Node<'a, 'dt>) -> Result<Self, Error> {
        if node.prop("interrupt-controller") != Some(&[][..])
            || property(node, "#interrupt-cells")? != 1
            || property(node, "#address-cells")? != 0
        {
            return Err(Error::Unsupported);
        }
        let reg = node.prop("reg").ok_or(Error::Missing)?;
        if reg.len() != 16 {
            return Err(Error::Unsupported);
        }
        let base = cells_u64(&reg[..8], 2).ok_or(Error::Malformed)?;
        let bytes = cells_u64(&reg[8..], 2).ok_or(Error::Malformed)?;
        let sources = property(node, "riscv,ndev")?;
        let phandle = property(node, "phandle")?;
        let extended = node.prop("interrupts-extended").ok_or(Error::Missing)?;
        if bytes == 0
            || base.checked_add(bytes).is_none()
            || sources == 0
            || sources > 1023
            || phandle == 0
            || phandle == u32::MAX
            || extended.is_empty()
            || !extended.len().is_multiple_of(8)
        {
            return Err(Error::Malformed);
        }
        let plic = Self {
            base,
            bytes,
            sources,
            phandle,
            extended,
            tree,
        };
        for context in plic.contexts() {
            context?;
        }
        Ok(plic)
    }
    pub fn contexts(&self) -> impl Iterator<Item = Result<Context, Error>> + '_ {
        self.extended
            .as_chunks::<8>()
            .0
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let phandle = word(entry, 0)?;
                let interrupt = word(entry, 4)?;
                // OpenSBI 将保留给 M 态的 context 标为 -1；该项仍占硬件序号。
                if interrupt != 9 && interrupt != 11 && interrupt != u32::MAX {
                    return Err(Error::Unsupported);
                }
                let cpus = self.tree.root().child("cpus").ok_or(Error::Missing)?;
                let width = property(&cpus, "#address-cells")?;
                if !(1..=2).contains(&width) {
                    return Err(Error::Unsupported);
                }
                let mut hartid = None;
                for cpu in cpus.children() {
                    if cpu.prop("device_type").and_then(property_string) != Some("cpu") {
                        continue;
                    }
                    for intc in cpu.children() {
                        if intc.prop_u32("phandle") != Some(phandle) {
                            continue;
                        }
                        if hartid.is_some() {
                            return Err(Error::Duplicate);
                        }
                        if node_status(&cpu).map_err(|_| Error::Malformed)? != NodeStatus::Okay
                            || node_status(&intc).map_err(|_| Error::Malformed)? != NodeStatus::Okay
                            || intc.prop("interrupt-controller") != Some(&[][..])
                            || property(&intc, "#interrupt-cells")? != 1
                            || !intc
                                .prop_str_list("compatible")
                                .is_some_and(|mut values| values.any(|v| v == "riscv,cpu-intc"))
                        {
                            return Err(Error::Unsupported);
                        }
                        let reg = cpu.prop("reg").ok_or(Error::Missing)?;
                        if reg.len() != width as usize * 4 {
                            return Err(Error::Malformed);
                        }
                        hartid = Some(cells_u64(reg, width as usize).ok_or(Error::Malformed)?);
                    }
                }
                Ok(Context {
                    index: u32::try_from(index).map_err(|_| Error::Malformed)?,
                    hartid: hartid.ok_or(Error::Missing)?,
                    interrupt,
                })
            })
    }
    /// 固定 virt PCI INTx 路由形状；返回 32..35，重复/遗漏或其他 interrupt parent 拒绝。
    pub fn virt_pci_sources(&self, pci: &Node<'_, '_>) -> Result<(u32, u32), Error> {
        let mask = pci.prop("interrupt-map-mask").ok_or(Error::Missing)?;
        if mask.len() != 16
            || word(mask, 0)? != 0x1800
            || word(mask, 4)? != 0
            || word(mask, 8)? != 0
            || word(mask, 12)? != 7
        {
            return Err(Error::Unsupported);
        }
        let map = pci.prop("interrupt-map").ok_or(Error::Missing)?;
        if map.len() != 16 * 24 {
            return Err(Error::Unsupported);
        }
        let mut seen = 0u16;
        for entry in map.as_chunks::<24>().0 {
            let device = word(entry, 0)?;
            let pin = word(entry, 12)?;
            if device & !0x1800 != 0
                || !(1..=4).contains(&pin)
                || word(entry, 4)? != 0
                || word(entry, 8)? != 0
                || word(entry, 16)? != self.phandle
                || word(entry, 20)? != 32 + ((device >> 11) + pin - 1) % 4
            {
                return Err(Error::Unsupported);
            }
            let bit = 1 << ((device >> 11) * 4 + pin - 1);
            if seen & bit != 0 {
                return Err(Error::Duplicate);
            }
            seen |= bit;
        }
        if seen != u16::MAX || self.sources < 35 {
            return Err(Error::Malformed);
        }
        Ok((32, 4))
    }
}
