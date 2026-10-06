//! 整空间退休 Work（内存分案设计轮次 3；进程回收 RetireSpace 阶段的执行者）。
//!
//! 随空间出生的独立宿主：记录、内嵌 [`Transport`] 与内联片段/退款游标
//! 随空间出生准入，退休时只激活，不做可失败准入。
//!
//! 前提：进程执行租约已离场、无在途 ChangeWork；退休不等待这些责任
//! 在本记录内自行完成。阶段：
//! 1. 进程 Frozen 阶段关闭入口并把当时的 active 快照交给本记录；
//!    收齐执行离场后激活，首步对该固定集合发整空间 fence。
//! 2. 全部 ack（fence 完成且槽归还）后才允许交出任何 View/use 或表帧。
//! 3. `Draining` 每步摘除固定数量的账本/页表单元，锁外释放固定批次；
//!    完成后交出 root 与账户。不对已隔离的空间重复发送 fence。

use alloc::sync::{Arc, Weak};

use erhino_shared::call::SystemCallError;
use maintenance::record::Outcome;

use super::{
    change::VmShell,
    remote::Transport,
    space::{DrainUnit, LEVELS, STEP_UNITS},
    views::{ViewRetirement, Views},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::memory_pool::MemoryPool,
};

struct State {
    phase: Phase,
    exit_mask: Option<u64>,
}

#[allow(
    clippy::large_enum_variant,
    reason = "退休游标在出生时内嵌，提交后阶段推进不得再分配"
)]
enum Phase {
    Idle,
    Advancing,
    ReadyToFence(u64),
    Fencing,
    Draining {
        cursor: page_table::DrainCursor<LEVELS>,
        permit: Option<memory_space::WritePermit>,
        fragment: Option<ViewRetirement>,
    },
    Done,
}

pub(crate) struct RetireWork {
    record: WorkRecord,
    this: Spinlock<Weak<RetireWork>>,
    shell: Arc<VmShell>,
    views: Arc<Views>,
    transport: Transport,
    /// 父锁只移交阶段/保护完成观察，执行 owner 在锁外直接推进退款。
    state: Spinlock<State>,
    completion: super::completion::Completion,
}

impl RetireWork {
    /// 随空间出生准入（可失败点在此，退休时不再分配）。
    pub(crate) fn new(
        pool: Arc<MemoryPool>,
        shell: Arc<VmShell>,
        views: Arc<Views>,
        source: Arc<crate::task::wait_source::SignalSource>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = crate::task::resources::MetadataCharge::arc::<Self>(&pool)?;
        let work = Arc::try_new(Self {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            shell,
            views,
            transport: Transport::new(),
            state: Spinlock::new(
                ranks::LEAF,
                State {
                    phase: Phase::Idle,
                    exit_mask: None,
                },
            ),
            completion: super::completion::Completion::new(metadata, None, source, true),
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *work.this.lock() = Arc::downgrade(&work);
        Ok(work)
    }

    /// Frozen 封口时接收入口门下的 active 快照，不能在延后的 activate 中重读。
    pub(crate) fn seal_exit(&self, active: u64) {
        let mut state = self.state.lock();
        assert!(
            matches!(state.phase, Phase::Idle),
            "exit sealed after retirement"
        );
        assert!(
            state.exit_mask.replace(active).is_none(),
            "exit sealed twice"
        );
    }

    /// 父回收只持独立完成来源，不保活私有执行宿主。
    pub(crate) fn completion(&self) -> Arc<crate::task::wait_source::SignalSource> {
        self.completion.source()
    }

    /// 激活退休（进程回收 RetireSpace 阶段调用一次）：入口已封且执行离场。
    pub(crate) fn activate(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            assert!(matches!(state.phase, Phase::Idle), "space retired twice");
            let mask = state
                .exit_mask
                .take()
                .expect("space retired without exit snapshot");
            let vm = self.shell.space.lock();
            assert!(vm.space.is_some(), "retire of a detached space");
            assert!(
                !vm.entry_open && !vm.access_open,
                "space retired before gates closed"
            );
            assert_eq!(vm.active, 0, "space retired with live execution leases");
            assert_eq!(vm.access_leases, 0, "space retired with live access leases");
            state.phase = Phase::ReadyToFence(mask);
        }
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }

    fn root(&self) -> Arc<dyn Work> {
        self.this
            .lock()
            .upgrade()
            .expect("retire work stepped after release")
    }

    fn step_fence(&self, phase: &mut Phase, root: &Arc<dyn Work>) -> Outcome {
        let mask = match phase {
            Phase::ReadyToFence(mask) => *mask,
            _ => unreachable!(),
        };
        let fence = {
            let vm = self.shell.space.lock();
            assert!(
                !vm.entry_open && !vm.access_open && vm.active == 0 && vm.access_leases == 0,
                "retire fence before quiescence"
            );
            self.shell.advance_translation()
        };
        let token = mt::arm(root);
        *phase = Phase::Fencing;
        if let Some(token) = self.transport.publish(mask, fence, token) {
            mt::wake(token);
        }
        Outcome::Blocked
    }

    fn begin_drain(&self, phase: &mut Phase) -> Outcome {
        assert!(
            self.transport.idle(),
            "retire drain before final fence acknowledgement"
        );
        let cursor = self
            .shell
            .space
            .lock()
            .space
            .as_ref()
            .expect("retire of a detached space")
            .begin_drain();
        *phase = Phase::Draining {
            cursor,
            permit: None,
            fragment: None,
        };
        Outcome::Ready
    }

    fn step_drain(&self, phase: &mut Phase) -> Outcome {
        let Phase::Draining {
            cursor,
            permit,
            fragment,
        } = phase
        else {
            unreachable!()
        };
        for _ in 0..STEP_UNITS {
            if let Some(current) = fragment.as_mut() {
                if self.views.release_fragment_step(current) {
                    fragment.take();
                }
                continue;
            }
            // 与该 view 关联的写许可在本记录完成退款之后归还。
            if let Some(permit) = permit.take() {
                self.views.release_permit(permit);
                continue;
            }
            let unit = {
                let mut vm = self.shell.space.lock();
                vm.space
                    .as_mut()
                    .expect("retire of a detached space")
                    .drain_step(cursor)
            };
            match unit {
                DrainUnit::Progress => {}
                DrainUnit::Fragment(next, owner) => {
                    *fragment = Some(ViewRetirement::new(next));
                    *permit = owner;
                }
                DrainUnit::Table(frame) => drop(frame),
                DrainUnit::Complete => {
                    assert!(self.views.is_empty(), "space retired before view resources");
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
                    *phase = Phase::Done;
                    return Outcome::Done;
                }
            }
        }
        Outcome::Ready
    }
}

impl Work for RetireWork {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(&self) -> Outcome {
        let root = self.root();
        let mut phase = {
            let mut state = self.state.lock();
            core::mem::replace(&mut state.phase, Phase::Advancing)
        };
        let outcome = match &mut phase {
            Phase::ReadyToFence(_) => self.step_fence(&mut phase, &root),
            Phase::Fencing => self.begin_drain(&mut phase),
            Phase::Draining { .. } => self.step_drain(&mut phase),
            Phase::Idle | Phase::Advancing => unreachable!("retire work has no executable phase"),
            Phase::Done => Outcome::Done,
        };
        let pending = (outcome == Outcome::Blocked && matches!(phase, Phase::Fencing))
            .then(|| self.transport.pending());
        {
            let mut state = self.state.lock();
            assert!(
                matches!(state.phase, Phase::Advancing),
                "retire phase moved twice"
            );
            state.phase = phase;
        }
        if let Some(mask) = pending {
            super::remote::kick(mask);
        }
        outcome
    }
}
