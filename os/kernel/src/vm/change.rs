//! ChangeWork：一次已提交地址空间修改的维护记录（内存分案设计轮次 3）。
//!
//! 阶段（单调）：
//! 1. `Publishing`：每步在空间锁内经 [`AnySpace::step`] 成对推进 ledger 与 PTE。
//! 2. 最后一次 PTE store 之后：最后离开者摘边界表进固定批次，取新 translation
//!    epoch（与责任提交身份分开），在空间锁内确定需覆盖的 active 目标，
//!    `maintenance::arm` 取唤醒根后经内嵌 [`Transport`] 发布；返回 Blocked。
//!    零目标不伪造 ack，同一完成谓词直接成立。
//! 3. `Retiring`：fence 全部确认（含槽归还）后，空间锁内 synchronize/complete
//!    账本；锁外每步放一张表帧或一个退休片段/许可，储备表页锁外退款。
//!
//! 锁序：change 状态（LEAF）→ 空间（ADDRESS_SPACE）→ 收件链（REMOTE_CALL）
//! → 维护队列（MAINTENANCE）；arm/wake 不在空间锁内调用。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicU64, Ordering};

use maintenance::record::Outcome;

use super::{
    remote::{Fence, Transport},
    space::{AnySpace, ChangeProgress, RetireBatch, STEP_UNITS},
};
use crate::{
    frame::FundedTableFrame,
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
};

/// 地址空间稳定外壳：identity 与 epoch 不随空间锁借用移动；`active` 由执行
/// 进入协议（entry 握手）在空间锁内维护，表示此前进入、可能使用旧翻译的 hart。
pub(crate) struct VmShell {
    pub(crate) identity: usize,
    pub(crate) translation_epoch: AtomicU64,
    pub(crate) instruction_epoch: AtomicU64,
    pub(crate) space: Spinlock<VmState>,
}

/// 空间锁保护的状态：账本/页表/账户与执行进入集合。
pub(crate) struct VmState {
    /// 整空间退休完成后取走（None 表示已退休，ChangeWork 不再可准入）。
    pub(crate) space: Option<AnySpace>,
    pub(crate) active: u64,
}

impl VmShell {
    /// 空间锁内调用：最后一次 PTE store 之后发布新代次，返回本批 fence 内容。
    pub(crate) fn advance_translation(&self) -> Fence {
        let translation = self.translation_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        Fence {
            space_identity: self.identity,
            translation_epoch: translation,
            instruction_epoch: self.instruction_epoch.load(Ordering::Acquire),
        }
    }
}

/// 退休片段/写许可的 backing 侧接收者（backing 层拥有区间退款算法）。
/// 每次调用有界；不得回调 ChangeWork。
pub(crate) trait RetireSink: Send + Sync {
    fn release_fragment(&self, fragment: memory_space::RetiringFragment);
    fn release_permit(&self, permit: memory_space::WritePermit);
}

struct State {
    phase: Phase,
    tables: RetireBatch,
}

enum Phase {
    Publishing(ChangeProgress),
    Fencing {
        committed: memory_space::CommittedChange,
        reserve: Vec<FundedTableFrame>,
    },
    Retiring {
        retiring: Option<memory_space::RetiringChange>,
        ledger_batch: memory_space::RetireBatch,
        reserve: Vec<FundedTableFrame>,
    },
    Done,
}

pub(crate) struct ChangeWork {
    record: WorkRecord,
    this: Weak<ChangeWork>,
    shell: Arc<VmShell>,
    sink: Arc<dyn RetireSink>,
    /// 出生时准入的内嵌运输与固定表帧批次（批间复用，不做可失败准入）。
    transport: Transport,
    /// 阶段与固定表帧批次同受一把锁：step 已由记录状态串行，批次不另设锁，
    /// 也避免在空间锁（ADDRESS_SPACE）内逆序获取 LEAF 锁。
    state: Spinlock<State>,
}

impl ChangeWork {
    /// 由已提交进度构造（可失败准入在发布预约之前完成，见 `AnySpace` 准备期）。
    pub(crate) fn new(
        account: Arc<mt::Account>,
        shell: Arc<VmShell>,
        sink: Arc<dyn RetireSink>,
        progress: ChangeProgress,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            record: WorkRecord::new(account),
            this: this.clone(),
            shell,
            sink,
            transport: Transport::new(),
            state: Spinlock::new(
                ranks::LEAF,
                State {
                    phase: Phase::Publishing(progress),
                    tables: RetireBatch::new(),
                },
            ),
        })
    }

    fn root(&self) -> Arc<dyn Work> {
        self.this.upgrade().expect("change work stepped after release")
    }

    /// 发布阶段一步；完成后进入 fence。
    fn step_publish(&self, phase: &mut Phase, tables: &mut RetireBatch) -> Outcome {
        let Phase::Publishing(progress) = phase else {
            unreachable!()
        };
        let mut vm = self.shell.space.lock();
        if !vm.space.as_mut().expect("change on a retired space").step(progress) {
            return Outcome::Ready;
        }
        // 最后一次 PTE store 已完成：摘边界表（同一锁内无间隙复检预约）。
        let range = progress.footprint();
                vm.space.as_mut().expect("change on a retired space").detach_boundary_tables(range, tables);
        let fence = self.shell.advance_translation();
        let mask = vm.active;
        drop(vm);
        let Phase::Publishing(progress) = core::mem::replace(phase, Phase::Done) else {
            unreachable!()
        };
        let (committed, reserve) = progress.finish();
        *phase = Phase::Fencing { committed, reserve };
        let token = mt::arm(&self.root());
        match self.transport.publish(mask, fence, token) {
            // 零目标：同一完成谓词立即成立，自唤醒（notified 覆盖 Blocked）。
            Some(token) => mt::wake(token),
            None => {}
        }
        Outcome::Blocked
    }

    /// fence 全部确认后：账本 synchronize/complete 前置的退休批次取出。
    fn step_synchronize(&self, phase: &mut Phase) -> Outcome {
        debug_assert!(self.transport.idle());
        let Phase::Fencing { committed, reserve } = core::mem::replace(phase, Phase::Done) else {
            unreachable!()
        };
        let mut vm = self.shell.space.lock();
        let (retiring, ledger_batch) = vm.space.as_mut().expect("change on a retired space").ledger_mut().synchronize(committed);
        drop(vm);
        *phase = Phase::Retiring {
            retiring: Some(retiring),
            ledger_batch,
            reserve,
        };
        Outcome::Ready
    }

    /// 锁外每步至多释放 [`STEP_UNITS`] 个 owner；全部归还后 complete。
    fn step_retire(&self, phase: &mut Phase, tables: &mut RetireBatch) -> Outcome {
        let Phase::Retiring {
            retiring,
            ledger_batch,
            reserve,
        } = phase
        else {
            unreachable!()
        };
        for _ in 0..STEP_UNITS {
            if let Some(frame) = tables.pop() {
                drop(frame);
            } else if let Some(fragment) = ledger_batch.pop_fragment() {
                self.sink.release_fragment(fragment);
            } else if let Some(permit) = ledger_batch.pop_permit() {
                self.sink.release_permit(permit);
            } else if let Some(frame) = reserve.pop() {
                drop(frame);
            } else {
                let retiring = retiring.take().expect("change completed twice");
                let mut vm = self.shell.space.lock();
                vm.space.as_mut().expect("change on a retired space").ledger_mut().complete(retiring, ledger_batch);
                drop(vm);
                *phase = Phase::Done;
                return Outcome::Done;
            }
        }
        Outcome::Ready
    }
}

impl Work for ChangeWork {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(&self, _grant: u64) -> (u64, Outcome) {
        let mut state = self.state.lock();
        let State { phase, tables } = &mut *state;
        let outcome = match phase {
            Phase::Publishing(_) => self.step_publish(phase, tables),
            Phase::Fencing { .. } => self.step_synchronize(phase),
            Phase::Retiring { .. } => self.step_retire(phase, tables),
            Phase::Done => Outcome::Done,
        };
        (STEP_UNITS as u64, outcome)
    }
}
