//! Hart 身份与启动发布（notes/impls/execution-context.md「Bootstrap 与正式环境」
//! 「身份、能力与拓扑」）。
//!
//! 三种事实互不混用：[`HartId`] 是 DT/SBI 的 raw hartid，可稀疏；
//! [`HartSlot`] 是内核按 admitted raw hartid 升序分配的稠密下标，HartLocal、
//! 栈与内部位图均按 slot 索引；拓扑（cpu-map）只服务 affinity/电源策略，
//! 不推断能力、不参与 slot 分配。SBI 边界显式转换回 raw hartid。

use core::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use crate::board::HartCapabilities;

/// 内核支持的 admitted hart 数上限（与链接脚本 HART_NUM_LIMIT 互校）。
pub const HART_NUM_LIMIT: usize = 8;

/// raw hartid：外部边界（DT reg / SBI 参数）使用。
pub type HartId = usize;

/// 稠密内核身份：admitted hart 按 raw hartid 升序分配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HartSlot(pub usize);

/// [`crate::hart::HartLocal`] 的静态数组容量校验由该模块的断言承担
/// （见 hart_local_slots）。

// ---------------------------------------------------------------------------
// HartBootRecord：每 admitted hart 的启动记录
// ---------------------------------------------------------------------------

/// 启动记录状态。每条 record 只有 Prepared → Starting → Online；
/// 全局启动失败后晚到 hart 只能看到 Failed gate 停驻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootState {
    /// record 已构造并完成数据发布（Release 前）。
    Prepared = 0,
    /// 已发出 HSM hart_start（或 boot hart 正在进入）。
    Starting = 1,
    /// formal entry 完成、CSR 基线建立、本 hart 已发布 Online。
    Online = 2,
}

/// 一条 admitted hart 的启动记录。
///
/// 发布协议：boot 以 `state.store(Starting, Release)` 发布后才发对应
/// `hart_start`；secondary PA 前导首先 `load(Acquire)` 看见 Starting 再读
/// 其余字段；formal entry 完成后 `store(Online, Release)`，boot 以 Acquire
/// 等待。要求平台对普通主存硬件 cache coherent（SMP 准入条件）。
#[repr(C)]
pub struct HartBootRecord {
    pub state: AtomicU8,
    /// 显式填充到 8 字节边界（PA 前导按固定偏移读取字段）。
    _pad0: [u8; 7],
    /// raw hartid（SBI hart_start 的目标；formal entry 回填 HartLocal 用）。
    pub hartid: HartId,
    /// 稠密身份（HartLocal 定位、栈选择）。
    pub slot: HartSlot,
    /// 正式内核 satp 值（含模式位）。
    pub kernel_satp: usize,
    /// 高半区 formal entry 的 VMA。
    pub entry_high: usize,
    /// 本 hart 的 HartLocal 地址（高半区 VA）。
    pub hart_local: usize,
    /// 本 hart 正式内核栈顶（高半区 VA，16-byte 对齐；emergency 区之下）。
    pub stack_top: usize,
    /// 本 hart emergency 栈顶（fatal 路径；正式栈区最高页）。
    pub emergency_sp: usize,
    /// 0 = secondary，1 = boot hart。
    pub role_boot: usize,
    _pad1: [usize; 2],
}

impl HartBootRecord {
    pub const fn zeroed() -> Self {
        Self {
            state: AtomicU8::new(BootState::Prepared as u8),
            _pad0: [0; 7],
            hartid: usize::MAX,
            slot: HartSlot(usize::MAX),
            kernel_satp: 0,
            entry_high: 0,
            hart_local: 0,
            stack_top: 0,
            emergency_sp: 0,
            role_boot: 0,
            _pad1: [0; 2],
        }
    }

    pub fn state(&self) -> BootState {
        match self.state.load(Ordering::Acquire) {
            2 => BootState::Online,
            1 => BootState::Starting,
            _ => BootState::Prepared,
        }
    }

    /// 发布进入 Starting（boot hart 发出 hart_start 前调用）。
    pub fn publish_starting(&self) {
        self.state
            .store(BootState::Starting as u8, Ordering::Release);
    }

    /// 发布 Online（formal entry 尾部调用）。
    pub fn publish_online(&self) {
        self.state.store(BootState::Online as u8, Ordering::Release);
    }
}

// PA 前导按固定偏移消费 record，布局即 ABI，全部字段显式锁定。
const _: () = assert!(core::mem::offset_of!(HartBootRecord, state) == 0);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, hartid) == 8);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, slot) == 16);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, kernel_satp) == 24);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, entry_high) == 32);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, hart_local) == 40);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, stack_top) == 48);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, emergency_sp) == 56);
const _: () = assert!(core::mem::offset_of!(HartBootRecord, role_boot) == 64);

// ---------------------------------------------------------------------------
// HartRegistry：双向 HartId ↔ HartSlot 映射 + record 存储
// ---------------------------------------------------------------------------

/// admitted hart 注册表。启动期单核构造，此后只读；
/// 查询接口可在任意 hart 上并发使用。
pub struct HartRegistry {
    /// slot 升序的 raw hartid 表（前 `len` 项有效）。
    ids: [HartId; HART_NUM_LIMIT],
    len: usize,
    records: [HartBootRecord; HART_NUM_LIMIT],
}

impl HartRegistry {
    /// 空 registry（启动早期占位；正式构造只能发生在 boot 单核阶段）。
    pub const fn empty() -> Self {
        Self {
            ids: [usize::MAX; HART_NUM_LIMIT],
            len: 0,
            records: [const { HartBootRecord::zeroed() }; HART_NUM_LIMIT],
        }
    }

    /// 登记一个 admitted hart（按升序遍历 DT CPU 的顺序即 slot 序）。
    /// 返回分配的 slot。
    pub fn admit(&mut self, id: HartId) -> HartSlot {
        assert!(
            self.len < HART_NUM_LIMIT,
            "admitted hart count exceeds limit"
        );
        let slot = HartSlot(self.len);
        self.ids[self.len] = id;
        let record = &mut self.records[self.len];
        record.hartid = id;
        record.slot = slot;
        self.len += 1;
        slot
    }

    /// raw → 稠密。未 admitted 的 hartid 返回 None。
    pub fn slot_of(&self, id: HartId) -> Option<HartSlot> {
        self.ids[..self.len]
            .iter()
            .position(|&x| x == id)
            .map(HartSlot)
    }

    /// 按 slot 访问启动记录。
    pub fn record(&self, slot: HartSlot) -> &HartBootRecord {
        &self.records[slot.0]
    }

    pub fn record_mut(&mut self, slot: HartSlot) -> &mut HartBootRecord {
        &mut self.records[slot.0]
    }

    /// 全部 record（slot 升序）。
    pub fn records(&self) -> impl Iterator<Item = (HartSlot, &HartBootRecord)> {
        self.records[..self.len]
            .iter()
            .enumerate()
            .map(|(i, r)| (HartSlot(i), r))
    }

    /// 全部 record 的可变访问（仅 boot 构造期使用）。
    pub fn records_mut(&mut self) -> impl Iterator<Item = (HartSlot, &mut HartBootRecord)> {
        self.records[..self.len]
            .iter_mut()
            .enumerate()
            .map(|(i, r)| (HartSlot(i), r))
    }

    /// 全部 admitted 稠密 slot 的固定宽位图。
    pub fn admitted_mask(&self) -> u64 {
        debug_assert!(self.len <= u64::BITS as usize);
        (1u64 << self.len) - 1
    }
}

// ---------------------------------------------------------------------------
// 全局注册表（boot 单核构造，此后只读）
// ---------------------------------------------------------------------------

use crate::sync::Spinlock;

static REGISTRY: Spinlock<Option<HartRegistry>> = Spinlock::new(crate::sync::ranks::LEAF, None);
/// 安装后不可变的 admitted slot 位图，供业务事务无锁验证 Remote Call 目标。
static ADMITTED_MASK: AtomicU64 = AtomicU64::new(0);

/// boot 构造完成后安装（只能发生一次）。
pub fn install(registry: HartRegistry) {
    let admitted = registry.admitted_mask();
    let mut guard = REGISTRY.lock();
    assert!(guard.is_none(), "registry already installed");
    *guard = Some(registry);
    assert_eq!(
        ADMITTED_MASK.swap(admitted, Ordering::Release),
        0,
        "admitted mask already published"
    );
}

/// 访问注册表（未安装即 panic——调用时序违约）。
pub fn with_registry<R>(f: impl FnOnce(&HartRegistry) -> R) -> R {
    f(REGISTRY.lock().as_ref().expect("registry not initialized"))
}

pub fn admitted_mask() -> u64 {
    let admitted = ADMITTED_MASK.load(Ordering::Acquire);
    assert!(admitted != 0, "registry not initialized");
    admitted
}

/// 把 slot 位图展开为 raw hartid 并逐个发送 IPI
/// （绝不把内部 slot 位图直接解释为 SBI hart mask）。
pub fn ipi_slots(mask: u64) {
    with_registry(|reg| {
        for (slot, record) in reg.records() {
            if mask & (1u64 << slot.0) != 0 {
                let raw = record.hartid;
                crate::sbi::require(crate::sbi::send_ipi(1, raw), "IPI.send");
            }
        }
    });
}

/// Remote Call 门铃：逐个发送并返回失败的稠密 slot 位图。请求 Pending 电平
/// 已在调用前发布，门铃失败不得撤销业务或伪造完成。
pub fn try_ipi_slots(mask: u64) -> u64 {
    with_registry(|reg| {
        let mut failed = 0;
        for (slot, record) in reg.records() {
            let bit = 1u64 << slot.0;
            if mask & bit != 0 && crate::sbi::send_ipi(1, record.hartid).is_err() {
                failed |= bit;
            }
        }
        failed
    })
}

// ---------------------------------------------------------------------------
// RuntimeGate：全局启动闸门
// ---------------------------------------------------------------------------

/// 全局运行时闸门。全体 admitted hart Online 后 boot 冻结 active 集合、
/// 完成调度域与初始任务装载，再以 Release 置 Ready；任何启动矛盾置 Failed。
/// 只有观察到 Ready 的 hart 可以进入调度器（防止 secondary 抢先触发静默判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GateState {
    Preparing = 0,
    Ready = 1,
    Failed = 2,
}

static GATE: AtomicU8 = AtomicU8::new(GateState::Preparing as u8);

pub fn gate_state() -> GateState {
    match GATE.load(Ordering::Acquire) {
        1 => GateState::Ready,
        2 => GateState::Failed,
        _ => GateState::Preparing,
    }
}

/// boot hart 冻结完成后发布 Ready。
pub fn publish_ready() {
    GATE.store(GateState::Ready as u8, Ordering::Release);
}

/// 任何启动错误使本次启动整体失败；不做部分降级。
pub fn publish_failed() {
    GATE.store(GateState::Failed as u8, Ordering::Release);
}

/// 在线 hart 在此处自旋等待 Ready/Failed 判定（Acquire 观察发布）。
/// 返回 true 表示可以进入调度器；false 表示启动失败，hart 应停驻等待复位。
pub fn wait_for_runtime() -> bool {
    loop {
        match gate_state() {
            GateState::Ready => return true,
            GateState::Failed => return false,
            GateState::Preparing => core::hint::spin_loop(),
        }
    }
}

// ---------------------------------------------------------------------------
// 能力快照（capability → domain eligibility 的输入）
// ---------------------------------------------------------------------------

/// 每 slot 的能力快照（registry 构造时从 DT 核验结果回填）。
pub static SLOT_CAPS: [AtomicUsize; HART_NUM_LIMIT] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; HART_NUM_LIMIT]
};

/// 把板级能力快照编码进 SLOT_CAPS[slot]。
pub fn store_caps(slot: HartSlot, caps: &HartCapabilities) {
    let mut bits = 0usize;
    if caps.f {
        bits |= 1;
    }
    if caps.d {
        bits |= 1 << 1;
    }
    if caps.q {
        bits |= 1 << 2;
    }
    if caps.v {
        bits |= 1 << 3;
    }
    SLOT_CAPS[slot.0].store(bits, Ordering::Release);
}

/// 读回 slot 的能力快照（boot 域构造用；编码与 store_caps 对偶）。
pub fn load_caps(slot: HartSlot) -> HartCapabilities {
    let bits = SLOT_CAPS[slot.0].load(Ordering::Acquire);
    HartCapabilities {
        f: bits & 1 != 0,
        d: bits & (1 << 1) != 0,
        q: bits & (1 << 2) != 0,
        v: bits & (1 << 3) != 0,
    }
}
