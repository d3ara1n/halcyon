//! 不反向保活业务对象的通用最终寿命观察。

use super::{
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    resources::{IpcPermit, MetadataSponsor},
    wait::Subscription,
};
use crate::sync::Spinlock;
use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
};

pub struct Lifetime {
    header: ObjectHeader,
    observed: u64,
    wait: Spinlock<ObjectWaitState>,
    _permit: IpcPermit,
}

pub struct LifetimeOwner(Arc<Lifetime>);

impl LifetimeOwner {
    pub fn new(
        observed: u64,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<(Self, ObjectRef), SystemCallError> {
        let state = Arc::try_new(Lifetime {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            observed,
            wait: Spinlock::new(
                crate::sync::ranks::LIFETIME,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok((Self(state.clone()), state))
    }
}

impl Drop for LifetimeOwner {
    fn drop(&mut self) {
        let pending = {
            let mut wait = self.0.wait.lock();
            wait.update(ObjectSignals::NONE, ObjectSignals::CLOSED);
            wait.take_notification()
        };
        if let Some((reservation, target)) = pending {
            reservation.publish(target)
        }
    }
}

impl KernelObject for Lifetime {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::Lifetime
    }
    fn related_id(&self) -> u64 {
        self.observed
    }
    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::LifetimeObserver)
            .then_some(Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::LifetimeObserver).then_some(ObjectSignals::CLOSED)
    }
    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }
    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }
    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.wait.lock().rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.wait.lock().cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.wait.lock().unsubscribe(id);
        drop(retired);
    }
    fn close_handle(&self, _: HandleRole, _: &Process, _: bool) {}
    fn close_transit(&self, _: HandleRole) {}
    fn advance_waiter(&self) -> super::object::WaitAdvance {
        self.wait.lock().advance_waiter()
    }
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.wait.lock().complete_notification(reservation)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
