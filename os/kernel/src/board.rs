//! 板级信息：从设备树就地解析 CPU、内存与 initfs 位置。
//!
//! 启动路径（帧池/堆就绪前）零堆依赖：结果存固定容量数组，
//! 容量即板级契约上限（HART_NUM_LIMIT / MAX_MEMORY_REGIONS），
//! 超出视为板级配置错误，启动期 panic。

use dtb::{cells_u64, Fdt};

use crate::hart::HART_NUM_LIMIT;

/// CPU 的 MMU 类型，`Bare` 表示不可运行本内核（无分页）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuType {
    Bare,
    Sv39,
    Sv48,
    Sv57,
}

#[derive(Clone, Copy)]
pub struct Cpu {
    pub hartid: usize,
    pub freq: usize,
    pub mmu: MmuType,
}

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub start: usize,
    pub len: usize,
}

/// memory 节点数上限（DTB 多段内存场景余量）。
const MAX_MEMORY_REGIONS: usize = 8;

pub struct BoardInfo {
    cpus: [Cpu; HART_NUM_LIMIT],
    cpu_len: usize,
    memories: [MemoryRegion; MAX_MEMORY_REGIONS],
    memory_len: usize,
    pub timebase: usize,
    pub initfs: Option<(usize, usize)>,
}

impl BoardInfo {
    pub fn cpus(&self) -> &[Cpu] {
        &self.cpus[..self.cpu_len]
    }

    pub fn memories(&self) -> &[MemoryRegion] {
        &self.memories[..self.memory_len]
    }
}

/// 父节点声明的 cells 宽度，缺省用 `default`。
fn cells(node: &dtb::Node, prop: &str, default: usize) -> usize {
    node.prop_u32(prop).map(|v| v as usize).unwrap_or(default)
}

/// 解析设备树。启动路径，遇到结构性缺失直接 panic（致命且不可恢复）。
pub fn parse(fdt: &Fdt) -> BoardInfo {
    let root = fdt.root();
    let (root_ac, root_sc) = (
        cells(&root, "#address-cells", 2),
        cells(&root, "#size-cells", 1),
    );

    // /chosen/initfs：cells 沿 chosen 覆盖继承自 root
    let mut initfs = None;
    if let Some(chosen) = root.child("chosen") {
        let (ac, sc) = (
            cells(&chosen, "#address-cells", root_ac),
            cells(&chosen, "#size-cells", root_sc),
        );
        if let Some(node) = chosen.child("initfs") {
            let reg = node.prop("reg").expect("initfs 节点缺 reg");
            let addr = cells_u64(reg, ac).expect("initfs reg 地址宽度异常") as usize;
            let len = cells_u64(&reg[ac * 4..], sc).expect("initfs reg 长度宽度异常") as usize;
            initfs = Some((addr, len));
        }
    }

    // /cpus：timebase + 每个 cpu@ 节点
    let mut timebase = 0;
    let mut cpus = [Cpu {
        hartid: 0,
        freq: 0,
        mmu: MmuType::Bare,
    }; HART_NUM_LIMIT];
    let mut cpu_len = 0;
    if let Some(cpus_node) = root.child("cpus") {
        timebase = cpus_node
            .prop_u32("timebase-frequency")
            .expect("cpus 节点缺 timebase-frequency") as usize;
        let ac = cells(&cpus_node, "#address-cells", 1);
        for cpu in cpus_node.children() {
            let name = cpu.name().expect("节点名不可达错误");
            if name.split('@').next() != Some("cpu") {
                continue;
            }
            let reg = cpu.prop("reg").expect("cpu 节点缺 reg");
            let mmu = match cpu.prop_str("mmu-type") {
                Some("riscv,sv39") => MmuType::Sv39,
                Some("riscv,sv48") => MmuType::Sv48,
                Some("riscv,sv57") => MmuType::Sv57,
                _ => MmuType::Bare,
            };
            let freq = match cpu.prop_u32("clock-frequency") {
                Some(f) => f as usize,
                None => timebase,
            };
            assert!(cpu_len < HART_NUM_LIMIT, "cpu 数超出 HART_NUM_LIMIT");
            cpus[cpu_len] = Cpu {
                hartid: cells_u64(reg, ac).expect("cpu reg 地址宽度异常") as usize,
                freq,
                mmu,
            };
            cpu_len += 1;
        }
    }
    assert!(cpu_len > 0, "设备树无可用 cpu 节点");

    // memory 节点（device_type == "memory"），reg 宽度按 root cells
    let mut memories = [MemoryRegion { start: 0, len: 0 }; MAX_MEMORY_REGIONS];
    let mut memory_len = 0;
    for node in root.children() {
        if node.prop_str("device_type") != Some("memory") {
            continue;
        }
        let reg = node.prop("reg").expect("memory 节点缺 reg");
        assert!(memory_len < MAX_MEMORY_REGIONS, "memory 节点数超上限");
        memories[memory_len] = MemoryRegion {
            start: cells_u64(reg, root_ac).expect("memory reg 地址宽度异常") as usize,
            len: cells_u64(&reg[root_ac * 4..], root_sc).expect("memory reg 长度宽度异常") as usize,
        };
        memory_len += 1;
    }
    assert!(memory_len > 0, "设备树无 memory 节点");

    BoardInfo {
        cpus,
        cpu_len,
        memories,
        memory_len,
        timebase,
        initfs,
    }
}
