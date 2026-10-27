//! PCI ECAM host 的定宽 DT 几何。这里只解析事实，不铸造物理访问 authority。
use crate::memory::PhysicalRange;
use crate::{Node, cells_u64, property_string};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Malformed,
    Unsupported,
    Overflow,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Io,
    Memory32,
    Memory64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub space: Space,
    pub prefetchable: bool,
    pub bus: PhysicalRange,
    pub cpu: PhysicalRange,
}
#[derive(Debug)]
pub struct Host<'a> {
    pub node_offset: usize,
    pub ecam: PhysicalRange,
    pub first_bus: u8,
    pub last_bus: u8,
    pub coherent: bool,
    ranges: &'a [u8],
    parent_cells: usize,
}
fn cell(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes.try_into().map_err(|_| Error::Malformed)?,
    ))
}
fn number(bytes: &[u8], cells: usize) -> Result<u64, Error> {
    cells_u64(bytes, cells).ok_or(Error::Malformed)
}
fn range(start: u64, bytes: u64) -> Result<PhysicalRange, Error> {
    if bytes == 0 {
        return Err(Error::Malformed);
    }
    Ok(PhysicalRange {
        start,
        end: start.checked_add(bytes).ok_or(Error::Overflow)?,
    })
}
impl<'a> Host<'a> {
    pub fn parse(
        node: &Node<'_, 'a>,
        parent_cells: usize,
        parent_size_cells: usize,
    ) -> Result<Self, Error> {
        if !(1..=2).contains(&parent_cells) || !(1..=2).contains(&parent_size_cells) {
            return Err(Error::Unsupported);
        }
        if node.prop("device_type").and_then(property_string) != Some("pci")
            || node.prop("#address-cells").map(cell).transpose()? != Some(3)
            || node.prop("#size-cells").map(cell).transpose()? != Some(2)
        {
            return Err(Error::Malformed);
        }
        let reg = node.prop("reg").ok_or(Error::Malformed)?;
        if reg.len() != (parent_cells + parent_size_cells) * 4 {
            return Err(Error::Malformed);
        }
        let ecam = range(
            number(&reg[..parent_cells * 4], parent_cells)?,
            number(&reg[parent_cells * 4..], parent_size_cells)?,
        )?;
        let buses = node.prop("bus-range").ok_or(Error::Malformed)?;
        if buses.len() != 8 {
            return Err(Error::Malformed);
        }
        let first = cell(&buses[..4])?;
        let last = cell(&buses[4..])?;
        if first > last
            || last > 255
            || !ecam.start.is_multiple_of(1 << 20)
            || !ecam.len().is_multiple_of(1 << 20)
            || ecam.len() < u64::from(last - first + 1) << 20
        {
            return Err(Error::Malformed);
        }
        let coherent = match node.prop("dma-coherent") {
            None => false,
            Some([]) => true,
            Some(_) => return Err(Error::Malformed),
        };
        let ranges = node.prop("ranges").ok_or(Error::Malformed)?;
        if ranges.is_empty() || !ranges.len().is_multiple_of((3 + parent_cells + 2) * 4) {
            return Err(Error::Malformed);
        }
        let host = Self {
            node_offset: node.offset(),
            ecam,
            first_bus: first as u8,
            last_bus: last as u8,
            coherent,
            ranges,
            parent_cells,
        };
        for window in host.windows() {
            window?;
        }
        Ok(host)
    }
    pub fn windows(&self) -> impl Iterator<Item = Result<Window, Error>> + '_ {
        self.ranges
            .chunks_exact((3 + self.parent_cells + 2) * 4)
            .map(|tuple| {
                let flags = cell(&tuple[..4])?;
                // 只支持无 BDF/register/relocatable 编码的 host ranges；配置空间不是 window。
                let space = match flags {
                    0x0100_0000 => Space::Io,
                    0x0200_0000 | 0x4200_0000 => Space::Memory32,
                    0x0300_0000 | 0x4300_0000 => Space::Memory64,
                    _ => return Err(Error::Unsupported),
                };
                let cpu_end = 12 + self.parent_cells * 4;
                let bytes = number(&tuple[cpu_end..], 2)?;
                let bus = range(number(&tuple[4..12], 2)?, bytes)?;
                let cpu = range(number(&tuple[12..cpu_end], self.parent_cells)?, bytes)?;
                if space != Space::Memory64 && bus.end > 1u64 << 32 {
                    return Err(Error::Overflow);
                }
                Ok(Window {
                    space,
                    prefetchable: flags & (1 << 30) != 0,
                    bus,
                    cpu,
                })
            })
    }
}
