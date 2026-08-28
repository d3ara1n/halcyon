//! 调度域推导：按「需求满足签名」划分 hart 域（纯逻辑，host 可测）。
//!
//! 域 = 满足同一组执行需求的 hart 等价类；与需求无关的能力差异不产生
//! 调度边界（方向公理见 notes/ideas/task.md「线程：执行单元」）。新执行
//! 需求加入 [`REQUIREMENTS`] 时划分按同一规则自动细化，细化只分裂既有
//! 域、不使既有绑定失效（细化后各成员仍满足原域全部需求）。
//!
//! 多域兼容时的默认放置政策：能力最弱的兼容域（满足需求集合最小），
//! 稀缺能力容量留给必须使用它的线程；显式 affinity 是未来的用户态政策。

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use elf::IsaRequirement;

/// hart 的用户可见持久状态扩展（DT 核验的硬件事实）。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HartCapabilities {
    /// F 扩展（单精度浮点）。
    pub f: bool,
    /// D 扩展（双精度浮点，蕴含 F）。
    pub d: bool,
    /// Q 扩展（四精度浮点）。
    pub q: bool,
    /// V 扩展（向量）。
    pub v: bool,
}

impl HartCapabilities {
    /// 有效 FLEN：无 F 为 0、F 为 32、D 为 64、Q 为 128。Q 不经「更宽」
    /// 排序表达——它是独立状态模型；因此 Q-capable hart（FLEN 128）不
    /// 兼容要求 FLEN 恰为 64 的 D64（Fp128 模型建立前，
    /// notes/impls/execution-context.md「内核与用户 ABI」）。
    pub fn flen(&self) -> usize {
        if self.q {
            128
        } else if self.d {
            64
        } else if self.f {
            32
        } else {
            0
        }
    }
}

/// 需求全集（位序即签名 mask 位）。新增执行需求档位时在此扩展，
/// 域划分随签名集合自动细化。
pub const REQUIREMENTS: [IsaRequirement; 2] = [IsaRequirement::Base64, IsaRequirement::D64];

/// 需求位序的人类可读名（日志/拓扑快照用；越界返回 "?"）。
pub fn requirement_label(index: usize) -> &'static str {
    match REQUIREMENTS.get(index) {
        Some(IsaRequirement::Base64) => "Base64",
        Some(IsaRequirement::D64) => "D64",
        _ => "?",
    }
}

/// hart 的需求满足签名：第 i 位 = 满足 [`REQUIREMENTS`]\[i\]。
fn signature(caps: &HartCapabilities) -> u32 {
    let mut mask = 0;
    for (i, requirement) in REQUIREMENTS.iter().enumerate() {
        if requirement.compatible(caps.flen()) {
            mask |= 1 << i;
        }
    }
    mask
}

/// 域划分结果：slot → 域下标 + 每域签名。域按 slot 升序首次出现编号。
pub struct DomainPlan {
    slot_domain: Vec<usize>,
    signatures: Vec<u32>,
}

impl DomainPlan {
    /// slot 的域下标（输入序即 slot 序）。
    pub fn slot_domain(&self, slot: usize) -> usize {
        self.slot_domain[slot]
    }

    /// 域数。
    pub fn domain_count(&self) -> usize {
        self.signatures.len()
    }

    /// 域的需求满足签名（位序同 [`REQUIREMENTS`]）。
    pub fn signature(&self, domain: usize) -> u32 {
        self.signatures[domain]
    }

    /// requirement 兼容域中最弱者（满足需求集合最小；同宽取首现序）。
    /// 无兼容域（含空划分）返回 None。
    pub fn resolve(&self, requirement: IsaRequirement) -> Option<usize> {
        let bit = REQUIREMENTS.iter().position(|r| *r == requirement)?;
        self.signatures
            .iter()
            .enumerate()
            .filter(|(_, sig)| *sig & (1 << bit) != 0)
            .min_by_key(|(index, sig)| (sig.count_ones(), *index))
            .map(|(index, _)| index)
    }
}

/// 按「需求满足签名」等价类划分（输入为 slot 序的 hart 能力）。
pub fn plan(caps: &[HartCapabilities]) -> DomainPlan {
    let mut slot_domain = Vec::with_capacity(caps.len());
    let mut signatures = Vec::new();
    for caps in caps {
        let sig = signature(caps);
        let index = signatures
            .iter()
            .position(|s| *s == sig)
            .unwrap_or_else(|| {
                signatures.push(sig);
                signatures.len() - 1
            });
        slot_domain.push(index);
    }
    DomainPlan { slot_domain, signatures }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(f: bool, d: bool, q: bool, v: bool) -> HartCapabilities {
        HartCapabilities { f, d, q, v }
    }

    /// 与调度无关的扩展差异（V）不产生域边界：同签名即同域。
    #[test]
    fn irrelevant_extension_does_not_split() {
        let p = plan(&[caps(true, true, false, true), caps(true, true, false, false)]);
        assert_eq!(p.domain_count(), 1);
        assert_eq!(p.slot_domain(0), 0);
        assert_eq!(p.slot_domain(1), 0);
    }

    #[test]
    fn heterogeneous_partitions_by_signature() {
        // slot 0/1/3 D-capable，slot 2 not：域 0 = {0,1,3}，域 1 = {2}。
        let p = plan(&[
            caps(true, true, false, false),
            caps(true, true, false, false),
            caps(false, false, false, false),
            caps(true, true, false, false),
        ]);
        assert_eq!(p.domain_count(), 2);
        assert_eq!(p.slot_domain(0), 0);
        assert_eq!(p.slot_domain(1), 0);
        assert_eq!(p.slot_domain(2), 1);
        assert_eq!(p.slot_domain(3), 0);
        // D64 只兼容域 0；Base64 默认落最弱兼容域（域 1 只满足 Base64）。
        assert_eq!(p.resolve(IsaRequirement::D64), Some(0));
        assert_eq!(p.resolve(IsaRequirement::Base64), Some(1));
    }

    #[test]
    fn q_hart_joins_base_domain_not_d64() {
        // Q（FLEN 128）与无 FP hart 同签名：都不满足 D64。
        let p = plan(&[caps(true, true, true, false), caps(true, true, false, false)]);
        assert_eq!(p.domain_count(), 2);
        assert_eq!(p.resolve(IsaRequirement::D64), Some(1));
        assert_eq!(p.resolve(IsaRequirement::Base64), Some(0));
    }

    #[test]
    fn d64_without_compatible_hart_resolves_none() {
        let p = plan(&[caps(false, false, false, false); 4]);
        assert_eq!(p.domain_count(), 1);
        assert_eq!(p.resolve(IsaRequirement::D64), None);
        // Base64 恒兼容（compatible 对任意 FLEN 为真），非空划分必有解。
        assert_eq!(p.resolve(IsaRequirement::Base64), Some(0));
    }

    /// Base64 在任意非空划分上可解（准入 ⇒ 基线 ⇒ Base64 兼容的
    /// 内核侧不变量在此表现为谓词恒真）。
    #[test]
    fn base64_always_resolvable_on_nonempty_plan() {
        for f in [false, true] {
            for d in [false, true] {
                for q in [false, true] {
                    let p = plan(&[caps(f, d, q, false)]);
                    assert_eq!(p.resolve(IsaRequirement::Base64), Some(0));
                }
            }
        }
    }

    #[test]
    fn empty_plan_resolves_none() {
        let p = plan(&[]);
        assert_eq!(p.domain_count(), 0);
        assert_eq!(p.resolve(IsaRequirement::Base64), None);
    }

    #[test]
    fn flen_orders_by_width_with_q_widest() {
        assert_eq!(caps(false, false, false, false).flen(), 0);
        assert_eq!(caps(true, false, false, false).flen(), 32);
        assert_eq!(caps(true, true, false, false).flen(), 64);
        assert_eq!(caps(true, true, true, false).flen(), 128);
        assert!(!IsaRequirement::D64.compatible(128));
        assert!(IsaRequirement::D64.compatible(64));
    }
}
