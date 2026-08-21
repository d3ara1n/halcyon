//! `/cpus/cpu-map` 拓扑解析（devicetree-schema cpu-map.yaml）。
//!
//! 层级 socket/cluster/core/thread 依命名约定 `socketN` 等编码；叶子
//! （无 SMT 的 core，或 SMT 下的 thread）经 `cpu` phandle 指向 cpu 节点。
//! 本模块只产出「phandle → 层级路径」的原始映射，不解释语义——slot
//! 分配与 affinity 策略属于内核（见 notes/execution-context.md「身份、
//! 能力与拓扑」）。

use alloc::{vec::Vec};

use crate::Node;

/// cpu-map 树中的一层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoLevel {
    Socket(u32),
    Cluster(u32),
    Core(u32),
    Thread(u32),
}

impl TopoLevel {
    /// 按 cpu-map 命名约定解析节点名为层级。非拓扑命名返回 `None`。
    pub fn from_name(name: &str) -> Option<Self> {
        let (kind, num) = name.split_at(name.find(|c: char| c.is_ascii_digit())?);
        let num: u32 = num.parse().ok()?;
        match kind {
            "socket" => Some(Self::Socket(num)),
            "cluster" => Some(Self::Cluster(num)),
            "core" => Some(Self::Core(num)),
            "thread" => Some(Self::Thread(num)),
            _ => None,
        }
    }
}

/// 一个叶子到 cpu phandle 的映射及其从 socket 起的完整层级路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopoLeaf {
    /// 叶子 `cpu` 属性指向的 phandle。
    pub cpu: u32,
    /// 从 socket 到叶子的路径。
    pub path: Vec<TopoLevel>,
}

/// 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoError {
    /// 同一 phandle 被映射多次。
    DuplicateCpu,
    /// 层级嵌套超出结构上限（cluster 可嵌套，深度仍有限）。
    TooDeep,
}

/// 层级深度上限（socket→cluster*→core→thread；schema 允许 cluster 嵌套，
/// 实际平台远小于此）。
const MAX_DEPTH: usize = 8;

/// 解析 `/cpus/cpu-map` 子树的全部叶子。`cpu_map` 为 cpu-map 节点；
/// 不存在时由调用方以 `None` 表达平坦拓扑。
pub fn parse(cpu_map: &Node<'_, '_>) -> Result<Vec<TopoLeaf>, TopoError> {
    let mut leaves = Vec::new();
    walk(cpu_map, Vec::new(), &mut leaves)?;
    Ok(leaves)
}

fn walk(
    node: &Node<'_, '_>,
    path: Vec<TopoLevel>,
    leaves: &mut Vec<TopoLeaf>,
) -> Result<(), TopoError> {
    if path.len() > MAX_DEPTH {
        return Err(TopoError::TooDeep);
    }
    for child in node.children() {
        let Some(level) = child.name().ok().and_then(TopoLevel::from_name) else {
            continue;
        };
        let mut path = path.clone();
        path.push(level);
        // core/thread 叶子带 `cpu` 属性；socket/cluster 是纯容器。
        if let Some(cpu) = child.prop_u32("cpu") {
            if leaves.iter().any(|l: &TopoLeaf| l.cpu == cpu) {
                return Err(TopoError::DuplicateCpu);
            }
            leaves.push(TopoLeaf { cpu, path });
        } else {
            walk(&child, path, leaves)?;
        }
    }
    Ok(())
}

/// 建立 `/cpus` 下 cpu 节点的 phandle → reg（raw hartid）映射。
/// 缺 reg 或 phandle 的 cpu 节点跳过——cpu-map 引用不到它即解析错误面。
pub fn cpu_phandle_hartids(cpus: &Node<'_, '_>, address_cells: usize) -> Vec<(u32, u64)> {
    let mut map = Vec::new();
    for node in cpus.children() {
        let is_cpu = node.name().is_ok_and(|n| n.split('@').next() == Some("cpu"));
        if !is_cpu {
            continue;
        }
        let (Some(phandle), Some(reg)) = (node.prop_u32("phandle"), node.prop("reg")) else {
            continue;
        };
        if let Some(hartid) = crate::cells_u64(reg, address_cells) {
            map.push((phandle, hartid));
        }
    }
    map
}
