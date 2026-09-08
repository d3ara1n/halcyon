//! 板级信息：从设备树就地解析 CPU、规范化物理供给与 BootPackage 窗口。
//!
//! 启动路径（帧池/堆就绪前）零堆依赖：结果存固定容量数组，容量即板级
//! 契约上限；超出、区间矛盾或尚无生命周期机制的 reserved-memory 语义均作为
//! 明确的平台 admission 失败。
//!
//! CPU 节点只接受现代 ISA 描述（`riscv,isa-base` + `riscv,isa-extensions`，
//! 见 references/normative/riscv-dt-bindings-linux-818bebeb/cpus.yaml）；
//! 已弃用的 `riscv,isa` 不解析。每个 hart 独立读取 status 与能力。

pub use dtb::cpu::MmuType;
use dtb::{
    Fdt, NodeStatus, cells_u64,
    cpu::parse as parse_platform_cpus,
    memory::{PhysicalRange, parse as parse_platform_memory},
    node_status, property_string,
    topology::{self, TopoLevel},
};
pub use sched_domain::HartCapabilities;

use crate::hart::HART_NUM_LIMIT;

#[derive(Clone, Copy)]
pub struct Cpu {
    pub hartid: usize,
    pub freq: usize,
    pub mmu: MmuType,
    pub caps: HartCapabilities,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: usize,
    pub len: usize,
}

impl MemoryRegion {
    const EMPTY: Self = Self { start: 0, len: 0 };

    pub fn end(self) -> usize {
        self.start
            .checked_add(self.len)
            .expect("physical memory range overflow")
    }

    fn overlaps(self, other: Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }
}

/// 规范化后 RAM extent 数量上限；按 reg tuple 而不是节点计数。
pub const MAX_MEMORY_REGIONS: usize = 16;
/// FDT reservation block 与静态 `/reserved-memory` 合并前的总 tuple 上限。
pub const MAX_PLATFORM_RESERVATIONS: usize = 32;
/// `no-map` 洞把一个连续直映射域最多切成 `N + 1` 段。
pub const MAX_DIRECT_MAP_REGIONS: usize = MAX_PLATFORM_RESERVATIONS + 1;
const PAGE_SIZE: usize = 4096;

struct BoardMemory {
    memories: [MemoryRegion; MAX_MEMORY_REGIONS],
    memory_len: usize,
    platform_reservations: [MemoryRegion; MAX_PLATFORM_RESERVATIONS],
    platform_reservation_len: usize,
    direct_map_regions: [MemoryRegion; MAX_DIRECT_MAP_REGIONS],
    direct_map_region_len: usize,
    dtb_range: MemoryRegion,
}

pub struct BoardInfo {
    cpus: [Cpu; HART_NUM_LIMIT],
    cpu_len: usize,
    memories: [MemoryRegion; MAX_MEMORY_REGIONS],
    memory_len: usize,
    platform_reservations: [MemoryRegion; MAX_PLATFORM_RESERVATIONS],
    platform_reservation_len: usize,
    direct_map_regions: [MemoryRegion; MAX_DIRECT_MAP_REGIONS],
    direct_map_region_len: usize,
    dtb_range: MemoryRegion,
    pub timebase: usize,
    /// 外部加载器提供的 BootPackage 物理窗口；初始为 DT capacity，
    /// envelope 校验后收窄为实际 total_len。
    pub boot_package: Option<(usize, usize)>,
    /// 可选 cpu-map 拓扑：(raw hartid, socket 起的层级路径)。
    /// 由 [`parse_topology`] 在帧池/堆就绪后填充。
    topology: Option<alloc::vec::Vec<(usize, alloc::vec::Vec<TopoLevel>)>>,
}

impl BoardInfo {
    pub fn cpus(&self) -> &[Cpu] {
        &self.cpus[..self.cpu_len]
    }

    pub fn memories(&self) -> &[MemoryRegion] {
        &self.memories[..self.memory_len]
    }

    pub fn platform_reservations(&self) -> &[MemoryRegion] {
        &self.platform_reservations[..self.platform_reservation_len]
    }

    pub fn direct_map_regions(&self) -> &[MemoryRegion] {
        &self.direct_map_regions[..self.direct_map_region_len]
    }

    pub fn dtb_range(&self) -> MemoryRegion {
        self.dtb_range
    }

    pub fn set_boot_package_len(&mut self, actual: usize) {
        let Some((address, capacity)) = self.boot_package else {
            panic!("BootPackage window unavailable");
        };
        assert!(
            actual > 0 && actual <= capacity,
            "BootPackage length exceeds DT window"
        );
        let range = page_cover(address, actual, "BootPackage range");
        assert!(
            contains_range(self.memories(), range),
            "BootPackage range lies outside DT memory"
        );
        assert!(
            !self.dtb_range.overlaps(range),
            "BootPackage range overlaps the device tree"
        );
        assert!(
            !self
                .platform_reservations()
                .iter()
                .any(|reserved| reserved.overlaps(range)),
            "BootPackage range overlaps permanent platform memory"
        );
        self.boot_package = Some((address, actual));
    }

    /// 平坦拓扑（cpu-map 缺省时）：全部 admitted hart 同属一个无层级集合。
    /// （准备态：affinity 策略接线后生效）
    #[expect(dead_code)]
    pub fn topology(&self) -> &[(usize, alloc::vec::Vec<TopoLevel>)] {
        self.topology.as_deref().unwrap_or(&[])
    }

    /// 帧池/堆就绪后解析 cpu-map（[`BoardInfo`] 构造期零堆约束之外）。
    /// 结果回填进自身；重复调用以最后一次为准。
    pub fn load_topology(&mut self, fdt: &Fdt) {
        let Some(cpus) = fdt.root().child("cpus") else {
            return;
        };
        let Some(map_node) = cpus.child("cpu-map") else {
            return;
        };
        let leaves = topology::parse(&map_node).expect("malformed cpu-map");
        let ac = cells(&cpus, "#address-cells", 1);
        let phandles = topology::cpu_phandle_hartids(&cpus, ac);
        self.topology = Some(
            leaves
                .into_iter()
                .map(|leaf| {
                    let hartid = phandles
                        .iter()
                        .find(|(ph, _)| *ph == leaf.cpu)
                        .map(|(_, hid)| *hid as usize)
                        .unwrap_or_else(|| {
                            panic!("cpu-map references unknown phandle {:#x}", leaf.cpu)
                        });
                    (hartid, leaf.path)
                })
                .collect(),
        );
    }
}

/// 父节点声明的 cells 宽度，缺省用 `default`。
fn cells(node: &dtb::Node, prop: &str, default: usize) -> usize {
    let width = match node.prop(prop) {
        None => default,
        Some(data) => usize::try_from(u32::from_be_bytes(
            data.try_into()
                .unwrap_or_else(|_| panic!("{prop} must contain exactly one cell")),
        ))
        .expect("cell width exceeds usize"),
    };
    assert!((1..=2).contains(&width), "{prop} has unsupported width");
    width
}

fn node_available(node: &dtb::Node<'_, '_>, label: &str) -> bool {
    match node_status(node) {
        Ok(NodeStatus::Okay) => true,
        Ok(NodeStatus::Disabled | NodeStatus::Reserved | NodeStatus::Failed) => false,
        Err(error) => panic!("{label} has invalid status: {error:?}"),
    }
}

fn from_physical(range: PhysicalRange) -> MemoryRegion {
    let start = usize::try_from(range.start).expect("physical address exceeds usize");
    let end = usize::try_from(range.end).expect("physical address exceeds usize");
    MemoryRegion {
        start,
        len: end - start,
    }
}

fn page_cover(start: usize, len: usize, label: &str) -> MemoryRegion {
    let end = start
        .checked_add(len)
        .unwrap_or_else(|| panic!("{label} overflows"));
    let aligned_end = end
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
        .unwrap_or_else(|| panic!("{label} alignment overflows"));
    MemoryRegion {
        start: start & !(PAGE_SIZE - 1),
        len: aligned_end - (start & !(PAGE_SIZE - 1)),
    }
}

fn contains_range(memories: &[MemoryRegion], range: MemoryRegion) -> bool {
    memories
        .iter()
        .any(|memory| memory.start <= range.start && range.end() <= memory.end())
}

fn build_direct_map_regions(
    no_map: &[PhysicalRange],
    end: usize,
    output: &mut [MemoryRegion; MAX_DIRECT_MAP_REGIONS],
) -> usize {
    let mut cursor = 0usize;
    let mut len = 0usize;
    for hole in no_map {
        let hole = from_physical(*hole);
        if cursor < hole.start.min(end) {
            output[len] = MemoryRegion {
                start: cursor,
                len: hole.start.min(end) - cursor,
            };
            len += 1;
        }
        cursor = cursor.max(hole.end().min(end));
        if cursor == end {
            break;
        }
    }
    if cursor < end {
        output[len] = MemoryRegion {
            start: cursor,
            len: end - cursor,
        };
        len += 1;
    }
    len
}

#[inline(never)]
fn parse_memory(fdt: &Fdt, dtb_pa: usize, boot_package: Option<(usize, usize)>) -> BoardMemory {
    let platform = parse_platform_memory::<MAX_MEMORY_REGIONS, MAX_PLATFORM_RESERVATIONS>(
        fdt,
        PAGE_SIZE as u64,
    )
    .unwrap_or_else(|error| panic!("platform memory description rejected: {error}"));

    let mut memories = [MemoryRegion::EMPTY; MAX_MEMORY_REGIONS];
    for (slot, range) in memories.iter_mut().zip(platform.memories()) {
        *slot = from_physical(*range);
    }
    let memory_len = platform.memories().len();

    let mut platform_reservations = [MemoryRegion::EMPTY; MAX_PLATFORM_RESERVATIONS];
    for (slot, range) in platform_reservations
        .iter_mut()
        .zip(platform.reservations())
    {
        *slot = from_physical(*range);
    }
    let platform_reservation_len = platform.reservations().len();

    let direct_map_end = memories[..memory_len]
        .iter()
        .map(|region| region.end())
        .max()
        .expect("device tree has no memory range");
    let mut direct_map_regions = [MemoryRegion::EMPTY; MAX_DIRECT_MAP_REGIONS];
    let direct_map_region_len =
        build_direct_map_regions(platform.no_map(), direct_map_end, &mut direct_map_regions);
    assert!(
        direct_map_region_len > 0,
        "reserved-memory no-map excludes the complete kernel direct-map domain"
    );

    let dtb_range = page_cover(dtb_pa, fdt.total_size(), "device tree range");
    assert!(
        contains_range(&memories[..memory_len], dtb_range),
        "device tree range lies outside DT memory"
    );
    assert!(
        contains_range(&direct_map_regions[..direct_map_region_len], dtb_range),
        "device tree range is excluded from the kernel direct map"
    );

    if let Some((address, capacity)) = boot_package {
        let range = page_cover(address, capacity, "boot-package physical window");
        assert!(
            contains_range(&memories[..memory_len], range),
            "boot-package physical window lies outside DT memory"
        );
        assert!(
            contains_range(&direct_map_regions[..direct_map_region_len], range),
            "boot-package physical window is excluded from the kernel direct map"
        );
    }

    BoardMemory {
        memories,
        memory_len,
        platform_reservations,
        platform_reservation_len,
        direct_map_regions,
        direct_map_region_len,
        dtb_range,
    }
}

/// 解析设备树。启动路径，遇到结构性缺失直接 panic（致命且不可恢复）。
pub fn parse(fdt: &Fdt, dtb_pa: usize) -> BoardInfo {
    let root = fdt.root();
    let (root_ac, root_sc) = (
        cells(&root, "#address-cells", 2),
        cells(&root, "#size-cells", 1),
    );

    // /chosen/boot-package：cells 沿 chosen 覆盖继承自 root。
    let mut boot_package = None;
    if let Some(chosen) = root.child("chosen") {
        let (ac, sc) = (
            cells(&chosen, "#address-cells", root_ac),
            cells(&chosen, "#size-cells", root_sc),
        );
        for node in chosen.children().filter(|node| {
            node.name()
                .is_ok_and(|name| name.split('@').next() == Some("boot-package"))
        }) {
            if !node_available(&node, "boot-package node") {
                continue;
            }
            assert!(
                boot_package.is_none(),
                "multiple available boot-package nodes"
            );
            assert!(
                node.prop("compatible").and_then(property_string) == Some("erhino,boot-package-v1"),
                "unsupported boot-package compatible"
            );
            let reg = node.prop("reg").expect("boot-package node missing reg");
            let address_bytes = ac.checked_mul(4).expect("boot-package reg width overflow");
            let total_bytes = ac
                .checked_add(sc)
                .and_then(|cells| cells.checked_mul(4))
                .expect("boot-package reg width overflow");
            assert_eq!(reg.len(), total_bytes, "malformed boot-package reg");
            let addr = usize::try_from(
                cells_u64(&reg[..address_bytes], ac)
                    .expect("unexpected boot-package reg address-cell width"),
            )
            .expect("boot-package address exceeds usize");
            let len = usize::try_from(
                cells_u64(&reg[address_bytes..], sc)
                    .expect("unexpected boot-package reg size-cell width"),
            )
            .expect("boot-package length exceeds usize");
            assert!(len != 0, "boot-package window is empty");
            boot_package = Some((addr, len));
        }
    }

    // /cpus：零分配 admission 一次冻结 status、能力、raw hartid、时钟与升序。
    let admitted_cpus = parse_platform_cpus::<HART_NUM_LIMIT>(fdt)
        .unwrap_or_else(|error| panic!("platform cpu description rejected: {error}"));
    let timebase = admitted_cpus.timebase_frequency() as usize;
    let mut cpus = [Cpu {
        hartid: 0,
        freq: 0,
        mmu: MmuType::Bare,
        caps: HartCapabilities::default(),
    }; HART_NUM_LIMIT];
    for (output, admitted) in cpus.iter_mut().zip(admitted_cpus.cpus()) {
        *output = Cpu {
            hartid: usize::try_from(admitted.hartid)
                .expect("admitted hartid exceeds runtime address width"),
            freq: admitted.frequency as usize,
            mmu: admitted.mmu,
            caps: HartCapabilities {
                f: admitted.capabilities.f,
                d: admitted.capabilities.d,
                q: admitted.capabilities.q,
                v: admitted.capabilities.v,
            },
        };
    }
    let cpu_len = admitted_cpus.cpus().len();
    // cpu-map 拓扑不在此解析：启动路径零堆，由 load_topology 在
    // 帧池/堆就绪后填充。

    // 平台 RAM、FDT reservation block、静态 /reserved-memory 与直映射 admission
    // 由独立零堆阶段完成；dynamic/reusable 在对应生命周期机制落地前 fail closed。
    let memory = parse_memory(fdt, dtb_pa, boot_package);

    BoardInfo {
        cpus,
        cpu_len,
        memories: memory.memories,
        memory_len: memory.memory_len,
        platform_reservations: memory.platform_reservations,
        platform_reservation_len: memory.platform_reservation_len,
        direct_map_regions: memory.direct_map_regions,
        direct_map_region_len: memory.direct_map_region_len,
        dtb_range: memory.dtb_range,
        timebase,
        boot_package,
        topology: None,
    }
}
