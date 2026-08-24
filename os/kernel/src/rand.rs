//! 内核随机数：非可预测 id 的唯一来源（隧道 id 等，见 notes/ideas/tunnel.md）。
//!
//! 分层设计：
//! - **主路（Zkr）**：DTB isa 声明 `_zkr` 时经 `seed` CSR（0x015）取硬件熵。
//!   访问纪律：必须用 csrrw 读写形式；每次返回 16bit，高两位 `OPST == ES16`
//!   才是有效熵。放行依赖现代 OpenSBI 检测到 Zkr 后置 `mseccfg.SSEED`
//!   （sbi_hart.c 现行行为）；若平台违约，首次读取将以非法指令显性失败——
//!   显性崩溃优于静默降级。
//! - **兜底（无 Zkr 平台，如 sifive_u）**：启动时刻的 rdtime 测量值过
//!   SplitMix64 finalizer 收敛。诚实声明：防碰撞足够，抗猜测弱于真熵；
//!   该路径平台的隧道 id 安全边界相应减弱（契约已声明安全边界本就不在
//!   机制层）。
//!
//! 输出流为 SplitMix64（state 单调步进 + finalizer 白化），Spinlock 护住；
//! 使用频率低（隧道创建级），无性能压力。

use core::{
    arch::asm,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::sync::Spinlock;

const SEED_CSR: u16 = 0x015;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

static STATE: Spinlock<u64> = Spinlock::new(0);
/// 兜底路径是否在用（观测用途；主路缺失时日志可见）。
static FALLBACK: AtomicU64 = AtomicU64::new(0);

/// SplitMix64 finalizer：白化单个 u64。
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 读一次 seed CSR（csrrw x0 形式），OPST == ES16 时返回 16bit 熵。
/// 注意：RV64 下 CSR 读数高位可能有杂散位，状态位须先掩码再判。
#[inline]
fn seed_entropy() -> Option<u16> {
    let v: usize;
    // SAFETY: 仅在 DTB 声明 _zkr 后触达；OpenSBI 已置 SSEED 放行 S-mode。
    unsafe {
        asm!(
            "csrrw {v}, {csr}, zero",
            v = out(reg) v,
            csr = const SEED_CSR,
            options(nomem)
        )
    }
    ((v >> 30) & 0b11 == 0b10).then_some((v & 0xFFFF) as u16)
}

/// 启动初始化（boot hart 调一次）。`has_zkr` 来自 DTB isa 解析；
/// `fallback_sample` 是兜底路径的启动时刻测量值（rdtime 等）。
pub fn init(has_zkr: bool, fallback_sample: u64) {
    let mut seed = mix(fallback_sample);
    let mut hw_bits = 0usize;
    if has_zkr {
        let mut acc = 0u64;
        for i in 0..4 {
            if let Some(v) = seed_entropy() {
                acc |= (v as u64) << (16 * i);
                hw_bits += 16;
            }
        }
        log!(Rand, "zkr seed: {} bit(s) harvested", hw_bits);
        seed ^= mix(acc);
    } else {
        log!(Rand, "no zkr on this platform, using mixed-boot-time fallback");
    }
    FALLBACK.store(u64::from(!has_zkr), Ordering::Relaxed);
    // 双重白化后入 state：即便样本低熵，finalizer 也保证输出分布均匀。
    *STATE.lock() = mix(seed ^ mix(fallback_sample.swap_bytes()));
}

/// 下一个 64bit 随机数。
pub fn next_u64() -> u64 {
    let mut s = STATE.lock();
    *s = s.wrapping_add(GOLDEN);
    mix(*s)
}

/// 下一个 48bit 随机数（隧道 id 空间）。
pub fn next_id48() -> u64 {
    next_u64() & 0xFFFF_FFFF_FFFF
}
