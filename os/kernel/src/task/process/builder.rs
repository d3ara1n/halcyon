//! 构造票据：最后 Pending authority 消散或显式 Abort 统一冻结 Shell。

use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, ObjectSignals, Rights},
    proc::{TerminationCause, TerminationDetail},
};

use super::super::{wait::WaitPlan, wait_source::SignalSource};
use super::{
    super::{
        job::Job,
        object::{KernelObject, ObjectHeader, ObjectRef},
    },
    shell::ProcessShell,
};
use crate::maintenance::{self as mt, Work, WorkRecord};
use crate::sync::{Spinlock, ranks};
use maintenance::record::Outcome;

/// 启动责任可直接授予或缓冲移动，始终单一authority，不允许复制。
pub(crate) const RIGHTS: Rights =
    Rights::from_raw(Rights::MANAGE.raw() | Rights::GRANT.raw() | Rights::TRANSIT.raw());

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Pending,
    Starting,
    Publishing,
    Started,
    Aborted,
}

struct State {
    phase: Phase,
    shell: Option<Arc<ProcessShell>>,
    cursor: usize,
    end: usize,
    tid_ceiling: erhino_shared::proc::Tid,
}

pub(crate) struct ProcessBuilder {
    header: ObjectHeader,
    holders: capability::Holders,
    process_id: u64,
    state: Spinlock<State>,
    record: WorkRecord,
    completion: BuilderCompletion,
}

struct BuilderCompletion {
    metadata: Option<super::super::resources::MetadataCharge>,
    receipt: Spinlock<Option<super::shell::WorkReceipt>>,
    source: Arc<SignalSource>,
}

impl Drop for BuilderCompletion {
    fn drop(&mut self) {
        drop(self.metadata.take());
        drop(self.receipt.get_mut().take());
        self.source.publish(ObjectSignals::DONE, false);
    }
}

impl ProcessBuilder {
    pub(crate) fn try_new(shell: Arc<ProcessShell>) -> Result<Arc<Self>, SystemCallError> {
        let pool = shell.funding_pool();
        let metadata = super::super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let builder = Arc::try_new(Self {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            process_id: shell.header.koid(),
            state: Spinlock::new(
                ranks::LEAF,
                State {
                    phase: Phase::Prepared,
                    shell: Some(shell),
                    cursor: 0,
                    end: 0,
                    tid_ceiling: 0,
                },
            ),
            completion: BuilderCompletion {
                metadata: Some(metadata),
                receipt: Spinlock::new(ranks::LEAF, None),
                source: SignalSource::try_new(pool.clone())?,
            },
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(builder)
    }

    pub(crate) fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        if object.kind() != ObjectKind::ProcessBuilder {
            return Err(SystemCallError::WrongObjectType);
        }
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    /// ProcessCreate 的对象出生已提交后、表项公开前 arm；失败路径不凭票据另启回收。
    pub(crate) fn arm(&self) {
        let mut state = self.state.lock();
        assert!(state.phase == Phase::Prepared, "builder armed twice");
        state.phase = Phase::Pending;
    }

    pub(crate) fn process(&self) -> Result<Arc<ProcessShell>, SystemCallError> {
        let state = self.state.lock();
        if state.phase != Phase::Pending {
            return Err(SystemCallError::ObjectNotAvailable);
        }
        Ok(state
            .shell
            .as_ref()
            .expect("Pending builder lost Shell")
            .clone())
    }

    pub(crate) fn start(self: &Arc<Self>) -> Result<WaitPlan, SystemCallError> {
        let shell = {
            let mut state = self.state.lock();
            if state.phase != Phase::Pending {
                return Err(SystemCallError::ObjectNotAvailable);
            }
            state.phase = Phase::Starting;
            state
                .shell
                .as_ref()
                .expect("Pending builder lost Shell")
                .clone()
        };
        let result = Job::start_commit_gate(&shell, shell.member_count());
        match result {
            Ok(body) => {
                // 归还指针借用，转为不借 Body 的已提交责任回执；
                // 私有启动宿主退款前不能解除 Frozen 的 ingress 屏障。
                *self.completion.receipt.lock() = Some(body.into_work_receipt());
                let wait = WaitPlan::completion(self.completion.source.clone());
                let root: Arc<dyn Work> = self.clone();
                mt::publish(root);
                mt::kick();
                Ok(wait)
            }
            Err(error) => {
                {
                    let mut state = self.state.lock();
                    assert!(
                        state.phase == Phase::Starting,
                        "builder start phase changed"
                    );
                    // Invocation 的消费 authority 仍在；失败先恢复 Pending，
                    // 随后的 restore/release 唯一决定继续持有还是放弃。
                    state.phase = Phase::Pending;
                }
                Err(error)
            }
        }
    }
}

impl KernelObject for ProcessBuilder {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::ProcessBuilder
    }
    fn related_id(&self) -> u64 {
        self.process_id
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn on_last_capability(&self) {
        let shell = {
            let mut state = self.state.lock();
            match state.phase {
                Phase::Pending => {
                    state.phase = Phase::Aborted;
                    state.shell.take()
                }
                Phase::Prepared => {
                    state.phase = Phase::Aborted;
                    let retained = state.shell.take();
                    drop(state);
                    drop(retained);
                    return;
                }
                // Starting 成功返回时 Work 已接管；authority 的最后消散不撤销它。
                Phase::Starting | Phase::Publishing | Phase::Started | Phase::Aborted => None,
            }
        };
        if let Some(shell) = shell {
            shell.freeze_and_seal(TerminationCause::Aborted, TerminationDetail::NONE);
        }
    }
}

impl Work for ProcessBuilder {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(self: Arc<Self>) -> Outcome {
        let (phase, shell, cursor, end, tid_ceiling) = {
            let state = self.state.lock();
            (
                state.phase,
                state.shell.as_ref().expect("start lost Shell").clone(),
                state.cursor,
                state.end,
                state.tid_ceiling,
            )
        };
        if let Some(body) = shell.lease_body() {
            match phase {
                Phase::Starting => {
                    let root: Arc<dyn Work> = self.clone();
                    let token = mt::arm(&root);
                    match body.vm.wait_start(token) {
                        None => return Outcome::Blocked,
                        Some(token) => drop(token),
                    }
                    let (end, tid_ceiling) = body.members.start_snapshot();
                    let mut state = self.state.lock();
                    state.end = end;
                    state.tid_ceiling = tid_ceiling;
                    state.phase = Phase::Publishing;
                    return Outcome::Ready;
                }
                Phase::Publishing if cursor < end => {
                    if let Some(thread) = body.members.start_created_at(cursor, tid_ceiling) {
                        thread.publish_ready();
                    }
                    self.state.lock().cursor += 1;
                    return Outcome::Ready;
                }
                Phase::Publishing => {}
                _ => unreachable!("uncommitted builder scheduled"),
            }
        }
        let retained = {
            let mut state = self.state.lock();
            state.phase = Phase::Started;
            state.shell.take()
        };
        drop(retained);
        Outcome::Done
    }
}
