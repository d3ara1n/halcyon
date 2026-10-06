//! 进程壳出生即持有的回收责任；每步只处理一个状态或固定数量的槽。

use alloc::sync::Arc;

use capability::TakeNext;
use erhino_shared::object::ObjectSignals;
use maintenance::record::Outcome;

use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    task::cap,
};

use super::{members::TerminationSlot, shell::ProcessShell};

const HANDLE_SLOTS_PER_STEP: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    WaitIngress,
    SweepMembers(usize),
    SweepReady(Option<usize>),
    WaitShell,
    BeginRetire,
    ReleaseHandles(usize),
    WaitVm,
    RetireSpace,
    WaitRetire,
    DropBody,
    Settle,
    Done,
}

pub(crate) struct ReclaimState {
    phase: Phase,
    retirement: Option<Arc<crate::task::wait_source::SignalSource>>,
}

impl ReclaimState {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            retirement: None,
        }
    }
}

impl ProcessShell {
    pub(crate) fn publish_reclaim(self: &Arc<Self>) {
        {
            let mut state = self.reclaim.lock();
            assert!(state.phase == Phase::Idle, "process reclaimed twice");
            state.phase = Phase::WaitIngress;
        }
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }

    fn root(&self) -> Arc<Self> {
        self.self_ref
            .lock()
            .upgrade()
            .expect("process work stepped after release")
    }

    fn step_wait_ingress(&self, root: &Arc<dyn Work>) -> Outcome {
        let token = mt::arm(root);
        if let Some(token) = self.wait_ingress(token) {
            drop(token);
            self.reclaim.lock().phase = Phase::SweepMembers(0);
            Outcome::Ready
        } else {
            Outcome::Blocked
        }
    }

    fn step_wait_shell(&self, root: &Arc<dyn Work>) -> Outcome {
        let token = mt::arm(root);
        if let Some(token) = self.wait_quiescent(token) {
            drop(token);
            self.reclaim.lock().phase = Phase::BeginRetire;
            Outcome::Ready
        } else {
            Outcome::Blocked
        }
    }

    fn step_begin_retire(&self) -> Outcome {
        let has_body = self.begin_retire();
        if has_body {
            self.with_retiring_body(|body| body.handles.lock().shut());
        }
        self.reclaim.lock().phase = if has_body {
            Phase::ReleaseHandles(0)
        } else {
            Phase::Settle
        };
        Outcome::Ready
    }

    fn step_release_handles(&self, mut cursor: usize) -> Outcome {
        let result = self.with_retiring_body(|body| {
            body.handles
                .lock()
                .take_next_bounded(&mut cursor, HANDLE_SLOTS_PER_STEP)
        });
        self.reclaim.lock().phase = if matches!(&result, TakeNext::Exhausted) {
            Phase::WaitVm
        } else {
            Phase::ReleaseHandles(cursor)
        };
        if let TakeNext::Cap(cap) = result {
            cap::deliver(cap.release());
        }
        Outcome::Ready
    }

    fn step_sweep_members(&self, shell: &Arc<Self>, cursor: usize) -> Outcome {
        let Some(body) = shell.lease_reclaim() else {
            self.reclaim.lock().phase = Phase::WaitShell;
            return Outcome::Ready;
        };
        let action = {
            if cursor >= body.members.slot_count() {
                self.reclaim.lock().phase = Phase::SweepReady(None);
                return Outcome::Ready;
            }
            body.members.termination_step(cursor)
        };
        drop(body);
        self.reclaim.lock().phase = Phase::SweepMembers(cursor + 1);
        match action {
            TerminationSlot::Created(thread) => {
                thread.finish_created_departure();
                drop(thread);
            }
            TerminationSlot::Waiting(owner) => crate::sched::reap(owner),
            TerminationSlot::Vacant
            | TerminationSlot::Ready
            | TerminationSlot::Running
            | TerminationSlot::Exiting => {}
        }
        Outcome::Ready
    }

    fn step_sweep_ready(&self, shell: &Arc<Self>, remaining: Option<usize>) -> Outcome {
        let domain = shell
            .lease_reclaim()
            .and_then(|body| *body.execution.lock());
        let Some(domain) = domain else {
            self.reclaim.lock().phase = Phase::WaitShell;
            return Outcome::Ready;
        };
        let remaining = remaining.unwrap_or_else(|| domain.ready_len());
        if remaining == 0 {
            self.reclaim.lock().phase = Phase::WaitShell;
            return Outcome::Ready;
        }
        let _ = domain.cancel_ready_step(self.pid);
        self.reclaim.lock().phase = Phase::SweepReady(Some(remaining - 1));
        Outcome::Ready
    }

    fn step_wait_vm(&self, root: &Arc<dyn Work>) -> Outcome {
        let token = mt::arm(root);
        let pending = self.with_retiring_body(|body| body.vm.wait_quiescent(token));
        if let Some(token) = pending {
            drop(token);
            self.reclaim.lock().phase = Phase::RetireSpace;
            Outcome::Ready
        } else {
            Outcome::Blocked
        }
    }

    fn step_retire(&self) -> Outcome {
        let retire = self.take_retirement();
        {
            let mut reclaim = self.reclaim.lock();
            reclaim.retirement = Some(retire.completion());
            reclaim.phase = Phase::WaitRetire;
        }
        retire.activate();
        Outcome::Ready
    }

    fn step_wait_retire(&self, root: &Arc<dyn Work>) -> Outcome {
        let token = mt::arm(root);
        let source = self
            .reclaim
            .lock()
            .retirement
            .as_ref()
            .expect("retire source lost")
            .clone();
        let pending = source.wait_done(token);
        if let Some(token) = pending {
            drop(token);
            let retired = {
                let mut reclaim = self.reclaim.lock();
                reclaim.phase = Phase::DropBody;
                reclaim.retirement.take()
            };
            // 完成来源也属于私有回收接线，Process DONE 前在锁外释放。
            drop(retired);
            Outcome::Ready
        } else {
            Outcome::Blocked
        }
    }

    fn step_drop_body(&self) -> Outcome {
        let body = self.take_body().expect("Body already released");
        drop(body);
        self.reclaim.lock().phase = Phase::Settle;
        Outcome::Ready
    }

    fn step_settle(&self) -> Outcome {
        self.mark_dead();
        self.source.publish(ObjectSignals::DONE, false);
        self.job
            .upgrade()
            .expect("process Job lost its member")
            .remove_member(self.pid);
        self.reclaim.lock().phase = Phase::Done;
        Outcome::Done
    }
}

impl Work for ProcessShell {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(&self) -> Outcome {
        let shell = self.root();
        let root: Arc<dyn Work> = shell.clone();
        let phase = self.reclaim.lock().phase;
        match phase {
            Phase::Idle => unreachable!("unpublished process work was scheduled"),
            Phase::WaitIngress => self.step_wait_ingress(&root),
            Phase::SweepMembers(cursor) => self.step_sweep_members(&shell, cursor),
            Phase::SweepReady(remaining) => self.step_sweep_ready(&shell, remaining),
            Phase::WaitShell => self.step_wait_shell(&root),
            Phase::BeginRetire => self.step_begin_retire(),
            Phase::ReleaseHandles(cursor) => self.step_release_handles(cursor),
            Phase::WaitVm => self.step_wait_vm(&root),
            Phase::RetireSpace => self.step_retire(),
            Phase::WaitRetire => self.step_wait_retire(&root),
            Phase::DropBody => self.step_drop_body(),
            Phase::Settle => self.step_settle(),
            Phase::Done => Outcome::Done,
        }
    }
}
