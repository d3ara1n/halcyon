//! 有界对象维护/退休执行者；拥有根与等待完成的 ticket 分离。

use super::{
    object::ObjectRef,
    proc::Process,
    wait::{WaitIdentity, WaitPlan},
};
use crate::{
    deferred_work::{Dependency, WakeAction},
    hart, registry,
    sync::Spinlock,
};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::call::SystemCallError;

const HARTS: usize = hart::HART_NUM_LIMIT;
const SLOTS: usize = super::resources::REGISTRATION_FINISH_LIMIT;
type Debts = work_debt::WorkDebts<ObjectRef, HARTS, SLOTS>;
static DEBTS: Spinlock<Debts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    Debts::new_with_id(work_debt::TableId::new(7)),
);
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

pub(crate) mod selftest;

pub(crate) trait RetirementTarget: Send + Sync {
    fn begin(
        &self,
        object: ObjectRef,
        owner: Option<Arc<Process>>,
    ) -> Result<Launch, SystemCallError>;
    fn step(&self, budget: usize) -> work_debt::StepResult<()>;
    fn register_progress(&self, wake: WakeAction);
    fn publish_progress(&self);
    fn finish(&self, reservation: Reservation, object: ObjectRef);
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

pub(crate) struct Reservation(Option<work_debt::Reservation>);

pub(crate) fn reserve() -> Result<Reservation, SystemCallError> {
    DEBTS
        .lock()
        .reserve()
        .map(|slot| Reservation(Some(slot)))
        .map_err(|_| SystemCallError::OutOfMemory)
}

impl Reservation {
    pub(crate) fn publish(mut self, object: ObjectRef) {
        let owner = hart::current().slot();
        let slot = self.0.take().expect("object maintenance published twice");
        {
            let mut debts = DEBTS.lock();
            debts
                .publish(slot, owner, object)
                .unwrap_or_else(|_| panic!("prepaid object maintenance must publish"));
            PENDING[owner].fetch_add(1, Ordering::Release);
        }
        ring(owner);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            assert!(
                DEBTS.lock().cancel(slot).is_ok(),
                "object maintenance reservation must refund"
            );
        }
    }
}

fn ring(owner: usize) {
    if registry::try_ipi_slots(1u64 << owner) != 0 {
        warn!(
            Task,
            "Object retirement doorbell failed for hart slot {owner}; work remains pending"
        );
    }
}

fn wake(token: work_debt::WakeToken) {
    let result = {
        let mut debts = DEBTS.lock();
        let result = debts
            .wake(token)
            .expect("object retirement wake lost its owner");
        if let work_debt::WakeResult::Runnable { owner } = result {
            PENDING[owner].fetch_add(1, Ordering::Release);
        }
        result
    };
    if let work_debt::WakeResult::Runnable { owner } = result {
        ring(owner);
    }
}

pub(crate) fn has_current() -> bool {
    PENDING[hart::current().slot()].load(Ordering::Acquire) != 0
}

fn return_slot(owner: usize, token: work_debt::FinishToken) -> Reservation {
    let mut debts = DEBTS.lock();
    let slot = debts
        .rearm(token)
        .expect("object maintenance must return its slot");
    assert!(
        PENDING[owner].fetch_sub(1, Ordering::AcqRel) > 0,
        "object maintenance must be pending"
    );
    Reservation(Some(slot))
}

pub(crate) fn drain_current(budget: usize) -> usize {
    let owner = hart::current().slot();
    let mut used = 0;
    while used < budget {
        let Some(taken) = DEBTS.lock().take(owner) else {
            break;
        };
        let (token, object) = taken.into_parts();
        let backend = object
            .retirement()
            .expect("queued object lost retirement backend");
        let advance = backend.step((budget - used).min(4));
        assert!(
            advance.work_done <= (budget - used).min(4),
            "object retirement exceeded its budget"
        );
        used += advance.work_done.max(1);
        match advance.state {
            work_debt::StepState::Complete => {
                let reservation = return_slot(owner, token);
                backend.finish(reservation, object.clone());
            }
            work_debt::StepState::Runnable => {
                assert!(
                    advance.work_done > 0,
                    "runnable object retirement made no progress"
                );
                DEBTS
                    .lock()
                    .requeue(token, object)
                    .unwrap_or_else(|_| panic!("object retirement must requeue"));
            }
            work_debt::StepState::Blocked(()) => {
                let ticket = DEBTS
                    .lock()
                    .arm_wake(&token)
                    .expect("object retirement must own its wake");
                backend.register_progress(WakeAction::unkeyed(ticket, wake));
                let mut debts = DEBTS.lock();
                if debts
                    .park(token, object)
                    .unwrap_or_else(|_| panic!("object retirement must park"))
                    == work_debt::ParkResult::Parked
                {
                    assert!(
                        PENDING[owner].fetch_sub(1, Ordering::AcqRel) > 0,
                        "parked retirement must be pending"
                    );
                }
            }
        }
    }
    if has_current() {
        ring(owner);
    }
    used
}

pub(crate) fn deliver_completion(reply: Option<WaitIdentity>, owner: Option<Arc<Process>>) {
    if let Some(owner) = owner
        && owner.lifecycle.complete_mandatory()
    {
        owner.publish_reapable();
    }
    if let Some(reply) = reply {
        reply.complete_kernel();
    }
}
