//! RISC-V CPU 节点的零分配 canonical admission。
//!
//! 输入只解释一次：节点状态、ISA/MMU 能力、raw hartid 与时钟在固定容量数组中
//! 排序去重，内核后续只消费本模块冻结的记录。

use core::{fmt, str};

use crate::{Fdt, NodeStatus, StatusError, cells_u64, node_status, property_string};

const BASELINE_EXTENSIONS: [&str; 6] = ["m", "a", "c", "zicsr", "zifencei", "zicntr"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuCapabilities {
    pub f: bool,
    pub d: bool,
    pub q: bool,
    pub v: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmuType {
    Bare,
    Sv39,
    Sv48,
    Sv57,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub hartid: u64,
    pub frequency: u32,
    pub mmu: MmuType,
    pub capabilities: CpuCapabilities,
}

impl Cpu {
    const EMPTY: Self = Self {
        hartid: 0,
        frequency: 0,
        mmu: MmuType::Bare,
        capabilities: CpuCapabilities {
            f: false,
            d: false,
            q: false,
            v: false,
        },
    };
}

pub struct CpuAdmission<const N: usize> {
    cpus: [Cpu; N],
    len: usize,
    timebase_frequency: u32,
}

impl<const N: usize> CpuAdmission<N> {
    pub fn cpus(&self) -> &[Cpu] {
        &self.cpus[..self.len]
    }

    pub const fn timebase_frequency(&self) -> u32 {
        self.timebase_frequency
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuError {
    MissingCpus,
    InvalidAddressCells,
    InvalidSizeCells,
    MissingTimebase,
    InvalidTimebase,
    MalformedStatus,
    UnknownStatus,
    MalformedNode,
    MalformedReg,
    HartIdOverflow,
    DuplicateHartId,
    CapacityExceeded,
    MissingIsaBase,
    UnsupportedIsaBase,
    MissingIsaExtensions,
    MalformedIsaExtensions,
    MissingBaselineExtension,
    InvalidCapabilityDependency,
    UnsupportedMmu,
    Empty,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingCpus => "device tree has no cpus node",
            Self::InvalidAddressCells => "cpus node has invalid address-cell width",
            Self::InvalidSizeCells => "cpus node has invalid size-cell width",
            Self::MissingTimebase => "cpus node is missing timebase-frequency",
            Self::InvalidTimebase => "cpus node has invalid timebase-frequency",
            Self::MalformedStatus => "cpu node has malformed status",
            Self::UnknownStatus => "cpu node has unknown status",
            Self::MalformedNode => "malformed cpu node",
            Self::MalformedReg => "cpu node has malformed reg",
            Self::HartIdOverflow => "cpu hartid exceeds the runtime address width",
            Self::DuplicateHartId => "duplicate cpu hartid",
            Self::CapacityExceeded => "admitted cpu count exceeds capacity",
            Self::MissingIsaBase => "cpu node is missing riscv,isa-base",
            Self::UnsupportedIsaBase => "cpu node has unsupported riscv,isa-base",
            Self::MissingIsaExtensions => "cpu node is missing riscv,isa-extensions",
            Self::MalformedIsaExtensions => "cpu node has malformed riscv,isa-extensions",
            Self::MissingBaselineExtension => "cpu node lacks a required baseline extension",
            Self::InvalidCapabilityDependency => "cpu capability dependency is invalid",
            Self::UnsupportedMmu => "cpu node has unsupported mmu-type",
            Self::Empty => "device tree has no admitted cpu nodes",
        };
        f.write_str(message)
    }
}

pub fn parse<const N: usize>(fdt: &Fdt<'_>) -> Result<CpuAdmission<N>, CpuError> {
    let cpus_node = fdt.root().child("cpus").ok_or(CpuError::MissingCpus)?;
    let address_cells = strict_u32(
        cpus_node
            .prop("#address-cells")
            .ok_or(CpuError::InvalidAddressCells)?,
    )
    .filter(|width| (1..=2).contains(width))
    .ok_or(CpuError::InvalidAddressCells)? as usize;
    if strict_u32(
        cpus_node
            .prop("#size-cells")
            .ok_or(CpuError::InvalidSizeCells)?,
    ) != Some(0)
    {
        return Err(CpuError::InvalidSizeCells);
    }
    let timebase_frequency = strict_u32(
        cpus_node
            .prop("timebase-frequency")
            .ok_or(CpuError::MissingTimebase)?,
    )
    .filter(|frequency| *frequency != 0)
    .ok_or(CpuError::InvalidTimebase)?;

    let mut result = CpuAdmission {
        cpus: [Cpu::EMPTY; N],
        len: 0,
        timebase_frequency,
    };
    for node in cpus_node.children() {
        let name = node.name().map_err(|_| CpuError::MalformedNode)?;
        if name.split('@').next() != Some("cpu") {
            continue;
        }
        match node_status(&node) {
            Ok(NodeStatus::Okay) => {}
            Ok(NodeStatus::Disabled | NodeStatus::Reserved | NodeStatus::Failed) => continue,
            Err(StatusError::Malformed) => return Err(CpuError::MalformedStatus),
            Err(StatusError::Unknown) => return Err(CpuError::UnknownStatus),
        }
        if node.prop("device_type").and_then(property_string) != Some("cpu") {
            return Err(CpuError::MalformedNode);
        }
        let reg = node.prop("reg").ok_or(CpuError::MalformedReg)?;
        if reg.len() != address_cells * 4 {
            return Err(CpuError::MalformedReg);
        }
        let hartid = cells_u64(reg, address_cells).ok_or(CpuError::MalformedReg)?;
        if usize::try_from(hartid).is_err() {
            return Err(CpuError::HartIdOverflow);
        }

        let base = node
            .prop("riscv,isa-base")
            .ok_or(CpuError::MissingIsaBase)?;
        if property_string(base).ok_or(CpuError::UnsupportedIsaBase)? != "rv64i" {
            return Err(CpuError::UnsupportedIsaBase);
        }
        let extensions = node
            .prop("riscv,isa-extensions")
            .ok_or(CpuError::MissingIsaExtensions)?;
        validate_string_list(extensions)?;
        for required in BASELINE_EXTENSIONS {
            if !contains_extension(extensions, required) {
                return Err(CpuError::MissingBaselineExtension);
            }
        }
        let capabilities = CpuCapabilities {
            f: contains_extension(extensions, "f"),
            d: contains_extension(extensions, "d"),
            q: contains_extension(extensions, "q"),
            v: contains_extension(extensions, "v"),
        };
        if (capabilities.d && !capabilities.f) || (capabilities.q && !capabilities.d) {
            return Err(CpuError::InvalidCapabilityDependency);
        }

        let mmu = match node.prop("mmu-type") {
            None => MmuType::Bare,
            Some(data) => match property_string(data).ok_or(CpuError::UnsupportedMmu)? {
                "riscv,none" => MmuType::Bare,
                "riscv,sv39" => MmuType::Sv39,
                "riscv,sv48" => MmuType::Sv48,
                "riscv,sv57" => MmuType::Sv57,
                _ => return Err(CpuError::UnsupportedMmu),
            },
        };
        let frequency = match node.prop("clock-frequency") {
            Some(data) => strict_u32(data).ok_or(CpuError::MalformedNode)?,
            None => timebase_frequency,
        };
        if result.len == N {
            return Err(CpuError::CapacityExceeded);
        }
        result.cpus[result.len] = Cpu {
            hartid,
            frequency,
            mmu,
            capabilities,
        };
        result.len += 1;
    }
    if result.len == 0 {
        return Err(CpuError::Empty);
    }
    result.cpus[..result.len].sort_unstable_by_key(|cpu| cpu.hartid);
    if result.cpus[..result.len]
        .windows(2)
        .any(|pair| pair[0].hartid == pair[1].hartid)
    {
        return Err(CpuError::DuplicateHartId);
    }
    Ok(result)
}

fn strict_u32(data: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = data.try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

fn validate_string_list(data: &[u8]) -> Result<(), CpuError> {
    if data.is_empty() || data.last() != Some(&0) {
        return Err(CpuError::MalformedIsaExtensions);
    }
    let mut rest = data;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(CpuError::MalformedIsaExtensions)?;
        if end == 0 || str::from_utf8(&rest[..end]).is_err() {
            return Err(CpuError::MalformedIsaExtensions);
        }
        let current = &rest[..end];
        let mut prior = data;
        while prior.as_ptr() != rest.as_ptr() {
            let prior_end = prior.iter().position(|byte| *byte == 0).unwrap();
            if prior_end == current.len() && &prior[..prior_end] == current {
                return Err(CpuError::MalformedIsaExtensions);
            }
            prior = &prior[prior_end + 1..];
        }
        rest = &rest[end + 1..];
    }
    Ok(())
}

fn contains_extension(data: &[u8], expected: &str) -> bool {
    data.split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .any(|value| value == expected.as_bytes())
}
