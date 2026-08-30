//! 设备树物理内存描述的零分配规范化。
//!
//! 本模块只解释平台交付的 RAM 与永久排除区，不掺入内核镜像、启动包、
//! 帧库存元数据等内核自有 reservation。调用方在发布库存前合并这些来源。

use core::fmt;

use crate::{Fdt, Node, cells_u64};

/// 已按页边界规范化的半开物理区间。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalRange {
    pub start: u64,
    pub end: u64,
}

impl PhysicalRange {
    const EMPTY: Self = Self { start: 0, end: 0 };

    pub const fn len(self) -> u64 {
        self.end - self.start
    }

    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
}

/// 平台物理内存描述错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMapError {
    InvalidPageSize,
    InvalidCellWidth,
    MalformedReg,
    EmptyRange,
    RangeOverflow,
    MemoryOverlap,
    CapacityExceeded,
    MalformedReservedMemory,
    UnsupportedDynamicReservation,
    UnsupportedReusable,
}

impl fmt::Display for MemoryMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPageSize => "invalid physical page size",
            Self::InvalidCellWidth => "unsupported device tree address or size cell width",
            Self::MalformedReg => "malformed device tree reg property",
            Self::EmptyRange => "physical memory range contains no complete page",
            Self::RangeOverflow => "physical memory range overflows",
            Self::MemoryOverlap => "device tree memory ranges overlap",
            Self::CapacityExceeded => "platform memory range capacity exceeded",
            Self::MalformedReservedMemory => "malformed reserved-memory description",
            Self::UnsupportedDynamicReservation => "dynamic reserved-memory is unsupported",
            Self::UnsupportedReusable => "reserved-memory reusable is unsupported",
        };
        f.write_str(message)
    }
}

/// 规范化的平台 RAM、全部排除区与其中禁止标准映射的子集。
pub struct PlatformMemory<const MEMORIES: usize, const RESERVATIONS: usize> {
    memories: [PhysicalRange; MEMORIES],
    memory_len: usize,
    reservations: [PhysicalRange; RESERVATIONS],
    reservation_len: usize,
    no_map: [PhysicalRange; RESERVATIONS],
    no_map_len: usize,
}

impl<const MEMORIES: usize, const RESERVATIONS: usize> PlatformMemory<MEMORIES, RESERVATIONS> {
    pub fn memories(&self) -> &[PhysicalRange] {
        &self.memories[..self.memory_len]
    }

    pub fn reservations(&self) -> &[PhysicalRange] {
        &self.reservations[..self.reservation_len]
    }

    pub fn no_map(&self) -> &[PhysicalRange] {
        &self.no_map[..self.no_map_len]
    }
}

/// 解析全部 `/memory` tuple、FDT reservation block 与静态 `/reserved-memory`。
///
/// `no-map` 同时进入普通分配排除集与禁止标准映射子集。`reusable` 需要 reclaim
/// owner，动态 reservation 需要启动期放置器；在相应机制存在前明确拒绝后两者。
pub fn parse<const MEMORIES: usize, const RESERVATIONS: usize>(
    fdt: &Fdt<'_>,
    page_size: u64,
) -> Result<PlatformMemory<MEMORIES, RESERVATIONS>, MemoryMapError> {
    if !page_size.is_power_of_two() {
        return Err(MemoryMapError::InvalidPageSize);
    }

    let root = fdt.root();
    let address_cells = cell_width(&root, "#address-cells", 2)?;
    let size_cells = cell_width(&root, "#size-cells", 1)?;

    let mut map = PlatformMemory {
        memories: [PhysicalRange::EMPTY; MEMORIES],
        memory_len: 0,
        reservations: [PhysicalRange::EMPTY; RESERVATIONS],
        reservation_len: 0,
        no_map: [PhysicalRange::EMPTY; RESERVATIONS],
        no_map_len: 0,
    };

    for node in root.children() {
        if node.prop_str("device_type") != Some("memory") || !is_available(&node) {
            continue;
        }
        let reg = node.prop("reg").ok_or(MemoryMapError::MalformedReg)?;
        each_reg(reg, address_cells, size_cells, |address, size| {
            let range = normalize_memory(address, size, page_size)?;
            push(&mut map.memories, &mut map.memory_len, range)
        })?;
    }
    if map.memory_len == 0 {
        return Err(MemoryMapError::EmptyRange);
    }
    normalize_memories(&mut map.memories, &mut map.memory_len)?;

    for reservation in fdt.memory_reservations() {
        let range = normalize_reservation(reservation.address, reservation.size, page_size)?;
        push(&mut map.reservations, &mut map.reservation_len, range)?;
    }

    if let Some(reserved) = root.child("reserved-memory") {
        parse_reserved_memory(
            &reserved,
            address_cells,
            size_cells,
            page_size,
            &mut map.reservations,
            &mut map.reservation_len,
            &mut map.no_map,
            &mut map.no_map_len,
        )?;
    }
    normalize_reservations(&mut map.reservations, &mut map.reservation_len);
    normalize_reservations(&mut map.no_map, &mut map.no_map_len);

    Ok(map)
}

fn parse_reserved_memory<const N: usize>(
    node: &Node<'_, '_>,
    root_address_cells: usize,
    root_size_cells: usize,
    page_size: u64,
    output: &mut [PhysicalRange; N],
    len: &mut usize,
    no_map_output: &mut [PhysicalRange; N],
    no_map_len: &mut usize,
) -> Result<(), MemoryMapError> {
    let address_cells = cell_width(node, "#address-cells", root_address_cells)?;
    let size_cells = cell_width(node, "#size-cells", root_size_cells)?;
    if address_cells != root_address_cells
        || size_cells != root_size_cells
        || node.prop("ranges") != Some(&[][..])
    {
        return Err(MemoryMapError::MalformedReservedMemory);
    }

    for child in node.children().filter(is_available) {
        let no_map = match child.prop("no-map") {
            Some(value) if value.is_empty() => true,
            Some(_) => return Err(MemoryMapError::MalformedReservedMemory),
            None => false,
        };
        let reusable = match child.prop("reusable") {
            Some(value) if value.is_empty() => true,
            Some(_) => return Err(MemoryMapError::MalformedReservedMemory),
            None => false,
        };
        if no_map && reusable {
            return Err(MemoryMapError::MalformedReservedMemory);
        }
        if reusable {
            return Err(MemoryMapError::UnsupportedReusable);
        }

        if let Some(reg) = child.prop("reg") {
            each_reg(reg, address_cells, size_cells, |address, size| {
                let range = normalize_reservation(address, size, page_size)?;
                push(output, len, range)?;
                if no_map {
                    push(no_map_output, no_map_len, range)?;
                }
                Ok(())
            })?;
        } else if child.prop("size").is_some() {
            return Err(MemoryMapError::UnsupportedDynamicReservation);
        } else {
            return Err(MemoryMapError::MalformedReservedMemory);
        }
    }
    Ok(())
}

fn is_available(node: &Node<'_, '_>) -> bool {
    node.prop("status").is_none() || matches!(node.prop_str("status"), Some("ok") | Some("okay"))
}

fn cell_width(
    node: &Node<'_, '_>,
    property: &str,
    default: usize,
) -> Result<usize, MemoryMapError> {
    let width = node
        .prop_u32(property)
        .map(|value| value as usize)
        .unwrap_or(default);
    if (1..=2).contains(&width) {
        Ok(width)
    } else {
        Err(MemoryMapError::InvalidCellWidth)
    }
}

fn each_reg(
    data: &[u8],
    address_cells: usize,
    size_cells: usize,
    mut emit: impl FnMut(u64, u64) -> Result<(), MemoryMapError>,
) -> Result<(), MemoryMapError> {
    let tuple_cells = address_cells
        .checked_add(size_cells)
        .ok_or(MemoryMapError::MalformedReg)?;
    let tuple_bytes = tuple_cells
        .checked_mul(4)
        .ok_or(MemoryMapError::MalformedReg)?;
    if data.is_empty() || data.len() % tuple_bytes != 0 {
        return Err(MemoryMapError::MalformedReg);
    }

    for tuple in data.chunks_exact(tuple_bytes) {
        let address = cells_u64(tuple, address_cells).ok_or(MemoryMapError::MalformedReg)?;
        let size = cells_u64(&tuple[address_cells * 4..], size_cells)
            .ok_or(MemoryMapError::MalformedReg)?;
        emit(address, size)?;
    }
    Ok(())
}

fn normalize_memory(
    address: u64,
    size: u64,
    page_size: u64,
) -> Result<PhysicalRange, MemoryMapError> {
    let end = address
        .checked_add(size)
        .ok_or(MemoryMapError::RangeOverflow)?;
    let start = align_up(address, page_size).ok_or(MemoryMapError::RangeOverflow)?;
    let end = align_down(end, page_size);
    if start >= end {
        return Err(MemoryMapError::EmptyRange);
    }
    Ok(PhysicalRange { start, end })
}

fn normalize_reservation(
    address: u64,
    size: u64,
    page_size: u64,
) -> Result<PhysicalRange, MemoryMapError> {
    if size == 0 {
        return Err(MemoryMapError::EmptyRange);
    }
    let end = address
        .checked_add(size)
        .ok_or(MemoryMapError::RangeOverflow)?;
    let start = align_down(address, page_size);
    let end = align_up(end, page_size).ok_or(MemoryMapError::RangeOverflow)?;
    Ok(PhysicalRange { start, end })
}

fn push<const N: usize>(
    output: &mut [PhysicalRange; N],
    len: &mut usize,
    range: PhysicalRange,
) -> Result<(), MemoryMapError> {
    let slot = output
        .get_mut(*len)
        .ok_or(MemoryMapError::CapacityExceeded)?;
    *slot = range;
    *len += 1;
    Ok(())
}

fn normalize_memories<const N: usize>(
    ranges: &mut [PhysicalRange; N],
    len: &mut usize,
) -> Result<(), MemoryMapError> {
    ranges[..*len].sort_unstable_by_key(|range| range.start);
    let mut output = 0usize;
    for input in 0..*len {
        let range = ranges[input];
        if output > 0 {
            let previous = &mut ranges[output - 1];
            if range.start < previous.end {
                return Err(MemoryMapError::MemoryOverlap);
            }
            if range.start == previous.end {
                previous.end = range.end;
                continue;
            }
        }
        ranges[output] = range;
        output += 1;
    }
    *len = output;
    Ok(())
}

fn normalize_reservations<const N: usize>(ranges: &mut [PhysicalRange; N], len: &mut usize) {
    ranges[..*len].sort_unstable_by_key(|range| range.start);
    let mut output = 0usize;
    for input in 0..*len {
        let range = ranges[input];
        if output > 0 {
            let previous = &mut ranges[output - 1];
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        ranges[output] = range;
        output += 1;
    }
    *len = output;
}

const fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment - 1)
        .map(|end| align_down(end, alignment))
}
