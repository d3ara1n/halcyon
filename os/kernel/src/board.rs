//! 板级信息：从设备树就地解析 CPU（现代 ISA 属性）、内存与 initfs 位置。
//!
//! 启动路径（帧池/堆就绪前）零堆依赖：结果存固定容量数组，
//! 容量即板级契约上限（HART_NUM_LIMIT / MAX_MEMORY_REGIONS），
//! 超出视为板级配置错误，启动期 panic。
//!
//! CPU 节点只接受现代 ISA 描述（`riscv,isa-base` + `riscv,isa-extensions`，
//! 见 references/normative/riscv-dt-bindings-linux-818bebeb/cpus.yaml）；
//! 已弃用的 `riscv,isa` 不解析。每个 hart 独立读取 status 与能力。

use dtb::{cells_u64, topology::{self, TopoLevel}, Fdt};

use crate::hart::HART_NUM_LIMIT;

/// 内核基线要求的扩展集合（`i` 由 isa-base rv64i 隐含）：
/// RV64IMAC + Zicsr + Zifencei + Zicntr。
const BASELINE_EXTENSIONS: [&str; 6] = ["m", "a", "c", "zicsr", "zifencei", "zicntr"];

/// hart 的用户可见持久状态扩展（DT 核验的硬件事实）。
#[derive(Clone, Copy, Default, Debug)]
pub struct HartCapabilities {
    /// F 扩展（单精度浮点）。
    pub f: bool,
    /// D 扩展（双精度浮点，蕴含 F）。
    pub d: bool,
    /// Q 扩展（四精度浮点）。
    pub q: bool,
    /// V 扩展（向量）。
    pub v: bool,
    /// Zkr 扩展（seed CSR 硬件熵源）。
    pub zkr: bool,
}

impl HartCapabilities {
    /// 有效 FLEN：无 F 为 0、F 为 32、D 为 64。Q 不经此表达——
    /// 它是独立状态模型，不是「更宽」的排序捷径。
    /// （准备态：domain eligibility 接线后生效）
    #[expect(dead_code)]
    pub fn flen(&self) -> usize {
        if self.d {
            64
        } else if self.f {
            32
        } else {
            0
        }
    }
}

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
    pub caps: HartCapabilities,
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
                        .unwrap_or_else(|| panic!("cpu-map references unknown phandle {:#x}", leaf.cpu));
                    (hartid, leaf.path)
                })
                .collect(),
        );
    }
}

/// 父节点声明的 cells 宽度，缺省用 `default`。
fn cells(node: &dtb::Node, prop: &str, default: usize) -> usize {
    node.prop_u32(prop).map(|v| v as usize).unwrap_or(default)
}

/// 就地查询现代 ISA 扩展列表是否含某项（零堆：不收集中间容器）。
fn has_extension(node: &dtb::Node, name: &str) -> bool {
    node.prop_str_list("riscv,isa-extensions")
        .is_some_and(|list| list.clone().any(|e| e == name))
}

/// 解析单个 cpu 节点为 [`Cpu`]；显式 disabled 返回 None（不准入）。
fn parse_cpu(node: &dtb::Node) -> Option<Cpu> {
    if node.prop_str("status").is_some_and(|s| s != "okay") {
        return None;
    }
    let reg = node.prop("reg")?;
    let hartid = cells_u64(reg, 1)? as usize;

    // 现代 ISA 属性是准入前提，缺失或基线不足属平台契约违约。
    let base = node
        .prop_str("riscv,isa-base")
        .unwrap_or_else(|| panic!("cpu {hartid} missing riscv,isa-base (legacy riscv,isa unsupported)"));
    assert!(base == "rv64i", "cpu {hartid} isa-base {base:?} is not rv64i");
    assert!(
        node.prop_str_list("riscv,isa-extensions").is_some(),
        "cpu {hartid} missing riscv,isa-extensions"
    );
    for required in BASELINE_EXTENSIONS {
        assert!(
            has_extension(node, required),
            "cpu {hartid} missing required baseline extension {required}"
        );
    }
    let caps = HartCapabilities {
        f: has_extension(node, "f"),
        d: has_extension(node, "d"),
        q: has_extension(node, "q"),
        v: has_extension(node, "v"),
        zkr: has_extension(node, "zkr"),
    };
    assert!(
        !(caps.d && !caps.f),
        "cpu {hartid} declares d but lacks f (binding constraint violated)"
    );

    let mmu = match node.prop_str("mmu-type") {
        Some("riscv,sv39") => MmuType::Sv39,
        Some("riscv,sv48") => MmuType::Sv48,
        Some("riscv,sv57") => MmuType::Sv57,
        _ => MmuType::Bare,
    };
    Some(Cpu { hartid, freq: 0, mmu, caps })
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
            let reg = node.prop("reg").expect("initfs node missing reg");
            let addr = cells_u64(reg, ac).expect("unexpected initfs reg address-cell width") as usize;
            let len = cells_u64(&reg[ac * 4..], sc).expect("unexpected initfs reg size-cell width") as usize;
            initfs = Some((addr, len));
        }
    }

    // /cpus：timebase + 每个 cpu@ 节点 + 可选 cpu-map
    let mut timebase = 0;
    let mut cpus = [Cpu {
        hartid: 0,
        freq: 0,
        mmu: MmuType::Bare,
        caps: HartCapabilities::default(),
    }; HART_NUM_LIMIT];
    let mut cpu_len = 0;
    if let Some(cpus_node) = root.child("cpus") {
        timebase = cpus_node
            .prop_u32("timebase-frequency")
            .expect("cpus node missing timebase-frequency") as usize;
        for cpu_node in cpus_node.children() {
            let name = cpu_node.name().expect("cpu node name unavailable");
            if name.split('@').next() != Some("cpu") {
                continue;
            }
            let Some(mut cpu) = parse_cpu(&cpu_node) else {
                continue;
            };
            cpu.freq = cpu_node.prop_u32("clock-frequency").map(|f| f as usize).unwrap_or(timebase);
            assert!(cpu_len < HART_NUM_LIMIT, "cpu count exceeds HART_NUM_LIMIT");
            cpus[cpu_len] = cpu;
            cpu_len += 1;
        }
        assert!(cpu_len > 0, "device tree has no usable cpu nodes");
        // cpu-map 拓扑不在此解析：启动路径零堆，由 load_topology 在
        // 帧池/堆就绪后填充。
    }

    // memory 节点（device_type == "memory"），reg 宽度按 root cells
    let mut memories = [MemoryRegion { start: 0, len: 0 }; MAX_MEMORY_REGIONS];
    let mut memory_len = 0;
    for node in root.children() {
        if node.prop_str("device_type") != Some("memory") {
            continue;
        }
        let reg = node.prop("reg").expect("memory node missing reg");
        assert!(memory_len < MAX_MEMORY_REGIONS, "memory node count exceeds limit");
        memories[memory_len] = MemoryRegion {
            start: cells_u64(reg, root_ac).expect("unexpected memory reg address-cell width") as usize,
            len: cells_u64(&reg[root_ac * 4..], root_sc).expect("unexpected memory reg size-cell width") as usize,
        };
        memory_len += 1;
    }
    assert!(memory_len > 0, "device tree has no memory node");

    BoardInfo {
        cpus,
        cpu_len,
        memories,
        memory_len,
        timebase,
        initfs,
        topology: None,
    }
}
