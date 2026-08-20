//! 板级信息：从设备树解析 CPU、内存与 initfs 位置。

use alloc::vec::Vec;
use dtb_parser::{
    prop::PropertyValue,
    traits::{FindPropertyValue, HasNamedChildNode, HasNamedProperty},
    DeviceTree,
};

/// CPU 的 MMU 类型，`Bare` 表示不可运行本内核（无分页）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuType {
    Bare,
    Sv39,
    Sv48,
    Sv57,
}

pub struct Cpu {
    pub hartid: usize,
    pub freq: usize,
    pub mmu: MmuType,
}

pub struct MemoryRegion {
    pub start: usize,
    pub len: usize,
}

pub struct BoardInfo {
    pub cpus: Vec<Cpu>,
    pub memories: Vec<MemoryRegion>,
    pub timebase: usize,
    pub initfs: Option<(usize, usize)>,
}

/// 解析设备树。启动路径，遇到结构性缺失直接 panic（致命且不可恢复）。
pub fn parse(tree: DeviceTree) -> BoardInfo {
    let mut cpus = Vec::new();
    let mut memories = Vec::new();
    let mut timebase = 0;
    let mut initfs = None;

    if let Some(chosen) = tree.find_node("/chosen/initfs") {
        if let Some(PropertyValue::Address(addr, len)) = chosen.value("reg") {
            initfs = Some((*addr as usize, *len as usize));
        }
    }

    if let Some(cpus_node) = tree.root().find_child("cpus") {
        if let Some(PropertyValue::Integer(tb)) = cpus_node.of_value("timebase-frequency") {
            timebase = *tb as usize;
        }
        for cpu in cpus_node
            .nodes()
            .iter()
            .filter(|node| node.type_name() == "cpu")
        {
            let PropertyValue::Address(hartid, _) = cpu.of_value("reg").expect("cpu 节点缺 reg") else {
                panic!("cpu 节点 reg 格式异常");
            };
            let mmu = match cpu.of_value("mmu-type") {
                Some(PropertyValue::String(mmu)) => match mmu.as_str() {
                    "riscv,sv39" => MmuType::Sv39,
                    "riscv,sv48" => MmuType::Sv48,
                    "riscv,sv57" => MmuType::Sv57,
                    _ => MmuType::Bare,
                },
                _ => MmuType::Bare,
            };
            let freq = match cpu.of_value("clock-frequency") {
                Some(PropertyValue::Integer(f)) => *f as usize,
                _ => timebase,
            };
            cpus.push(Cpu {
                hartid: *hartid as usize,
                freq,
                mmu,
            });
        }
    }

    for node in tree.root().nodes() {
        let is_memory =
            matches!(node.of_value("device_type"), Some(PropertyValue::String(s)) if s == "memory");
        if !is_memory {
            continue;
        }
        let PropertyValue::Address(addr, len) = node.of_value("reg").expect("memory 节点缺 reg")
        else {
            panic!("memory 节点 reg 格式异常");
        };
        memories.push(MemoryRegion {
            start: *addr as usize,
            len: *len as usize,
        });
    }

    BoardInfo {
        cpus,
        memories,
        timebase,
        initfs,
    }
}
