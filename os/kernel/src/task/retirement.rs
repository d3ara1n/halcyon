//! 有界对象维护/退休执行者；拥有根与等待完成的 ticket 分离。

use super::{
    object::ObjectRef,
    proc::Process,
    wait::{WaitIdentity, WaitPlan},
};
use crate::{
    deferred_work::{Dependency, WakeAction},
    hart,
    work_ledger::{DebtLedger, Reservation as LedgerReservation},
};
use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;

const SLOTS: usize = super::resources::REGISTRATION_FINISH_LIMIT;
type Debts = DebtLedger<ObjectRef, SLOTS>;
static DEBTS: Debts = Debts::new(work_debt::TableId::new(7));

pub(crate) mod selftest;

/// 已提交对象退休的稳定内部票据。票据持有对象强引用，只暴露完成依赖，
/// 不把 RetirementTarget 的执行容量和回复责任泄漏给请求层。
#[derive(Clone)]
pub(crate) struct RetirementTicket {
    object: ObjectRef,
}

pub(crate) struct RetirementCompletion {
    pub(crate) reply: Option<WaitIdentity>,
    pub(crate) owner: Option<Arc<Process>>,
}

impl RetirementCompletion {
    pub(crate) fn deliver(self) {
        if let Some(owner) = self.owner
            && owner.lifecycle.complete_mandatory()
        {
            owner.publish_reapable();
        }
        if let Some(reply) = self.reply {
            reply.complete_kernel();
        }
    }
}

impl RetirementTicket {
    pub(crate) fn new(object: ObjectRef) -> Self {
        assert!(
            object.retirement().is_some(),
            "retirement ticket requires a retirement backend"
        );
        Self { object }
    }

    pub(crate) fn register(&self, action: WakeAction, cancelled: impl FnOnce() -> bool) {
        let target = self
            .object
            .retirement()
            .expect("retirement ticket lost its backend");
        target
            .completion()
            .register(action, || target.is_finished() || cancelled());
    }

    pub(crate) fn cancel(&self, key: crate::deferred_work::WaitKey) {
        self.object
            .retirement()
            .expect("retirement ticket lost its backend")
            .completion()
            .cancel_keyed(key);
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.object
            .retirement()
            .expect("retirement ticket lost its backend")
            .is_finished()
    }
}

pub(crate) trait RetirementTarget: Send + Sync {
    fn begin(
        &self,
        object: ObjectRef,
        owner: Option<Arc<Process>>,
    ) -> Result<Launch, SystemCallError>;
    fn step(&self, budget: usize) -> work_debt::StepResult<()>;
    fn register_progress(&self, wake: WakeAction);
    fn publish_progress(&self);
    fn finish(&self, reservation: Reservation, object: ObjectRef) -> Option<RetirementCompletion>;
    fn completion(&self) -> &Dependency;
    fn is_finished(&self) -> bool;
}

pub(crate) struct Launch {
    pub(crate) object: ObjectRef,
    pub(crate) work: Option<Reservation>,
    pub(crate) reply: Option<WaitPlan>,
}

impl Launch {
    pub(crate) fn publish(self) -> Option<WaitPlan> {
        let backend = self
            .object
            .retirement()
            .expect("retirement launch lost its backend");
        backend.publish_progress();
        if let Some(work) = self.work {
            work.publish(self.object);
        }
        self.reply
    }
}

pub(crate) type Reservation = LedgerReservation<ObjectRef, SLOTS>;

pub(crate) fn reserve() -> Result<Reservation, SystemCallError> {
    DEBTS.reserve().map_err(|_| SystemCallError::OutOfMemory)
}

fn wake(token: work_debt::WakeToken) {
    DEBTS.wake_raw(token);
}

pub(crate) fn has_current() -> bool {
    DEBTS.pending(hart::current().slot()) != 0
}

fn return_slot(token: super::super::work_ledger::Token<ObjectRef, SLOTS>) -> Reservation {
    token.rearm()
}

pub(crate) fn drain_current(budget: usize) -> usize {
    let owner = hart::current().slot();
    let mut used = 0;
    while used < budget {
        let Some(taken) = DEBTS.take(owner) else {
            break;
        };
        let (token, object) = taken.into_parts();
        let backend = object
            .retirement()
            .expect("queued object lost retirement backend");
        let advance =
            backend.step((budget - used).min(crate::work_ledger::MAX_STEPS_PER_DEBT_TURN));
        assert!(
            advance.work_done <= (budget - used).min(crate::work_ledger::MAX_STEPS_PER_DEBT_TURN),
            "object retirement exceeded its budget"
        );
        used += advance.work_done.max(1);
        match advance.state {
            work_debt::StepState::Complete => {
                let reservation = return_slot(token);
                if let Some(completion) = backend.finish(reservation, object.clone()) {
                    completion.deliver();
                }
            }
            work_debt::StepState::Runnable => {
                assert!(
                    advance.work_done > 0,
                    "runnable object retirement made no progress"
                );
                token.requeue(object);
            }
            work_debt::StepState::Blocked(()) => {
                let ticket = token
                    .arm_wake()
                    .expect("object retirement must own its wake");
                backend.register_progress(WakeAction::unkeyed(ticket.into_raw(), wake));
                token.park(object);
            }
        }
    }
    DEBTS.ring_if_pending(owner);
    used
}
