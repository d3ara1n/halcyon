//! 整空间退休 Work（内存分案设计轮次 3；进程回收 RetireSpace 阶段的执行者）。
//!
//! 出生内嵌于地址空间外壳：记录、内嵌 [`Transport`] 与固定 [`RetireBatch`]
//! 随空间出生准入，退休时只激活，不做可失败准入。
//!
//! 前提：执行进入已封闭（active 不再增长），无在途 ChangeWork（Settle 前由
//! 进程回收保证）。阶段：
//! 1. `Draining`：空间锁内每步至多 [`STEP_UNITS`] 个单元；账本片段锁外交
//!    [`RetireSink`]；摘出的表帧进批次。批满或 drain 完成时 →
//! 2. `Fencing`：取新 epoch，对 active 发 fence（零目标同谓词直接成立）；
//!    全部确认（fence 完成 ∧ 槽归还）后锁外放本批表帧、槽位复用，回 1。
//! 3. drain 完成且最后一批已放：交出 root 与账户，锁外退款，Done。

use alloc::sync::{Arc, Weak};

use maintenance::record::Outcome;

use super::{
    change::{RetireSink, VmShell},
    remote::Transport,
    space::{DrainUnit, LEVELS, RetireBatch, STEP_UNITS},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
};

struct State {
    phase: Phase,
    tables: RetireBatch,
}

enum Phase {
    Idle,
    Draining {
        cursor: page_table::DrainCursor<LEVELS>,
        complete: bool,
    },
    Fencing {
        cursor: page_table::DrainCursor<LEVELS>,
        complete: bool,
    },
    Done,
}

pub(crate) struct RetireWork {
    record: WorkRecord,
    this: Weak<RetireWork>,
    shell: Arc<VmShell>,
    sink: Arc<dyn RetireSink>,
    transport: Transport,
    /// 阶段与固定表帧批次同受一把锁：step 已由记录状态串行，批次不另设锁，
    /// 也避免在空间锁（ADDRESS_SPACE）内逆序获取 LEAF 锁。
    state: Spinlock<State>,
}

impl RetireWork {
    /// 随空间出生准入（可失败点在此，退休时不再分配）。
    pub(crate) fn new(
        account: Arc<mt::Account>,
        shell: Arc<VmShell>,
        sink: Arc<dyn RetireSink>,
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
                    phase: Phase::Idle,
                    tables: RetireBatch::new(),
                },
            ),
        })
    }

    /// 激活退休（进程回收 RetireSpace 阶段调用一次）：Idle → Draining 并发布。
    pub(crate) fn activate(self: &Arc<Self>) {
        let cursor = self
            .shell
            .space
            .lock()
            .space
            .as_ref()
            .expect("retire of a detached space")
            .begin_drain();
        {
            let mut state = self.state.lock();
            assert!(matches!(state.phase, Phase::Idle), "space retired twice");
            state.phase = Phase::Draining {
                cursor,
                complete: false,
            };
        }
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }

    fn root(&self) -> Arc<dyn Work> {
        self.this.upgrade().expect("retire work stepped after release")
    }

    fn step_drain(&self, phase: &mut Phase, tables: &mut RetireBatch) -> Outcome {
        let Phase::Draining { cursor, complete } = phase else {
            unreachable!()
        };
        let mut fragments = [const { None }; STEP_UNITS];
        let mut taken = 0;
        {
            let mut vm = self.shell.space.lock();
            let space = vm.space.as_mut().expect("retire of a detached space");
            while taken < STEP_UNITS && !*complete && !tables.is_full() {
                match space.drain_step(cursor) {
                    DrainUnit::Progress => {}
                    DrainUnit::Fragment(fragment, permit) => {
                        fragments[taken] = Some((fragment, permit));
                    }
                    DrainUnit::Table(frame) => tables.push(frame),
                    DrainUnit::Complete => *complete = true,
                }
                taken += 1;
            }
        }
        // 锁外交 backing 层：片段区间退款与写许可归还。
        for (fragment, permit) in fragments.iter_mut().filter_map(Option::take) {
            self.sink.release_fragment(fragment);
            if let Some(permit) = permit {
                self.sink.release_permit(permit);
            }
        }
        let batch_ready = tables.is_full() || *complete;
        if !batch_ready {
            return Outcome::Ready;
        }
        // 摘表后取新 epoch：本批 fence 覆盖父 PTE 撤销。
        let (fence, mask) = {
            let vm = self.shell.space.lock();
            (self.shell.advance_translation(), vm.active)
        };
        let Phase::Draining { cursor, complete } = core::mem::replace(phase, Phase::Done) else {
            unreachable!()
        };
        *phase = Phase::Fencing { cursor, complete };
        let token = mt::arm(&self.root());
        if let Some(token) = self.transport.publish(mask, fence, token) {
            mt::wake(token);
        }
        Outcome::Blocked
    }

    fn step_release(&self, phase: &mut Phase, tables: &mut RetireBatch) -> Outcome {
        debug_assert!(self.transport.idle());
        for _ in 0..STEP_UNITS {
            let Some(frame) = tables.pop() else {
                break;
            };
            drop(frame);
        }
        if !tables.is_empty() {
            return Outcome::Ready;
        }
        let Phase::Fencing { cursor, complete } = core::mem::replace(phase, Phase::Done) else {
            unreachable!()
        };
        if !complete {
            *phase = Phase::Draining { cursor, complete };
            return Outcome::Ready;
        }
        // 全部表帧已在 fence 后释放：交出 root 与账户锁外退款。
        let space = self
            .shell
            .space
            .lock()
            .space
            .take()
            .expect("retire of a detached space");
        let (root, account) = space.finish_drain();
        drop(root);
        drop(account);
        Outcome::Done
    }
}

impl Work for RetireWork {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(&self, _grant: u64) -> (u64, Outcome) {
        let mut state = self.state.lock();
        let State { phase, tables } = &mut *state;
        let outcome = match phase {
            Phase::Draining { .. } => self.step_drain(phase, tables),
            Phase::Fencing { .. } => self.step_release(phase, tables),
            Phase::Idle | Phase::Done => Outcome::Done,
        };
        (STEP_UNITS as u64, outcome)
    }
}
