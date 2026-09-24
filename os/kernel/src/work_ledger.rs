//! 内核固定容量债务账本。
//!
//! 每张业务债务表保留自己的 payload、容量、owner FIFO 与推进策略；
//! 本模块只拥有表锁、Pending 电平、票据归属、槽位退款和 park/wake
//! 状态转换。业务推进与来源条件始终在账本锁外执行。

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{hart, registry, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;

/// 所有安全点共享的总预算与单债务 turn；领域只负责自己的容量预留。
pub(crate) const MAX_STEPS_PER_SAFE_POINT: usize = 16;
pub(crate) const MAX_STEPS_PER_DEBT_TURN: usize = 4;

pub(crate) fn safe_point_budget(pending: [bool; 4]) -> work_debt::FairBudget<4> {
    work_debt::FairBudget::new(MAX_STEPS_PER_SAFE_POINT, MAX_STEPS_PER_DEBT_TURN, pending)
}

pub(crate) struct DebtLedger<T, const SLOTS: usize> {
    debts: Spinlock<work_debt::WorkDebts<T, HARTS, SLOTS>>,
    pending: [AtomicUsize; HARTS],
}

impl<T, const SLOTS: usize> DebtLedger<T, SLOTS> {
    pub(crate) const fn new(table_id: work_debt::TableId) -> Self {
        Self {
            debts: Spinlock::new(
                crate::sync::ranks::WORK_DEBT,
                work_debt::WorkDebts::new_with_id(table_id),
            ),
            pending: [const { AtomicUsize::new(0) }; HARTS],
        }
    }
}

impl<T: Send + 'static, const SLOTS: usize> DebtLedger<T, SLOTS> {
    pub(crate) fn reserve(&'static self) -> Result<Reservation<T, SLOTS>, work_debt::ReserveError> {
        self.reserve_in(0..SLOTS)
    }

    pub(crate) fn reserve_in(
        &'static self,
        range: core::ops::Range<usize>,
    ) -> Result<Reservation<T, SLOTS>, work_debt::ReserveError> {
        let reservation = self.debts.lock().reserve_in(range)?;
        Ok(Reservation {
            ledger: self,
            reservation: Some(reservation),
        })
    }

    pub(crate) fn take(&'static self, owner: usize) -> Option<Taken<T, SLOTS>> {
        let taken = self.debts.lock().take(owner)?;
        let (token, value) = taken.into_parts();
        Some(Taken {
            ledger: self,
            token,
            value,
        })
    }

    fn publish(
        &'static self,
        reservation: work_debt::Reservation,
        owner: usize,
        value: T,
        ring: bool,
    ) {
        let result = {
            let mut debts = self.debts.lock();
            let result = debts.publish(reservation, owner, value);
            if result.is_ok() {
                self.pending[owner].fetch_add(1, Ordering::Release);
            }
            result
        };
        match result {
            Ok(()) => {}
            Err(error) => {
                let (_, value) = error.into_parts();
                drop(value);
                panic!("reserved work debt slot must publish");
            }
        }
        if ring {
            self.ring(owner);
        }
    }

    fn cancel(&'static self, reservation: work_debt::Reservation) {
        assert!(
            self.debts.lock().cancel(reservation).is_ok(),
            "reserved work debt slot must roll back"
        );
    }

    fn requeue(&'static self, token: work_debt::FinishToken, value: T) {
        let result = {
            let mut debts = self.debts.lock();
            debts.requeue(token, value)
        };
        if let Err(error) = result {
            let (_, value) = error.into_parts();
            drop(value);
            panic!("taken work debt must requeue");
        }
    }

    fn park(&'static self, token: work_debt::FinishToken, value: T) -> work_debt::ParkResult {
        let owner = token.owner();
        let result = {
            let mut debts = self.debts.lock();
            let result = debts.park(token, value);
            if matches!(result, Ok(work_debt::ParkResult::Parked)) {
                self.decrement_pending(owner);
            }
            result
        };
        match result {
            Ok(result) => result,
            Err(error) => {
                let (_, value) = error.into_parts();
                drop(value);
                panic!("taken work debt must park");
            }
        }
    }

    fn finish(&'static self, token: work_debt::FinishToken) {
        let owner = token.owner();
        let mut debts = self.debts.lock();
        debts
            .finish(token)
            .unwrap_or_else(|_| panic!("taken work debt must finish"));
        self.decrement_pending(owner);
    }

    fn rearm(&'static self, token: work_debt::FinishToken) -> Reservation<T, SLOTS> {
        let owner = token.owner();
        let mut debts = self.debts.lock();
        let reservation = debts
            .rearm(token)
            .unwrap_or_else(|_| panic!("taken work debt must rearm"));
        self.decrement_pending(owner);
        drop(debts);
        Reservation {
            ledger: self,
            reservation: Some(reservation),
        }
    }

    fn arm_wake(&'static self, token: &work_debt::FinishToken) -> Option<WakeToken<T, SLOTS>> {
        self.debts.lock().arm_wake(token).map(|wake| WakeToken {
            wake,
            _marker: core::marker::PhantomData,
        })
    }

    pub(crate) fn wake_raw(&'static self, wake: work_debt::WakeToken) -> work_debt::WakeResult {
        let result = {
            let mut debts = self.debts.lock();
            let result = debts
                .wake(wake)
                .unwrap_or_else(|_| panic!("work debt wake lost its owner"));
            if let work_debt::WakeResult::Runnable { owner } = result {
                self.pending[owner].fetch_add(1, Ordering::Release);
            }
            result
        };
        if let work_debt::WakeResult::Runnable { owner } = result {
            self.ring(owner);
        }
        result
    }

    pub(crate) fn has_pending(&'static self, owner: usize) -> bool {
        self.debts.lock().has_pending(owner)
    }

    pub(crate) fn pending(&'static self, owner: usize) -> usize {
        self.pending[owner].load(Ordering::Acquire)
    }

    pub(crate) fn available(&'static self) -> usize {
        self.debts.lock().available()
    }

    fn decrement_pending(&'static self, owner: usize) {
        assert!(
            self.pending[owner].fetch_sub(1, Ordering::AcqRel) > 0,
            "work debt transition must own a pending slot"
        );
    }

    pub(crate) fn ring_if_pending(&'static self, owner: usize) {
        if self.pending(owner) != 0 {
            self.ring(owner);
        }
    }

    fn ring(&'static self, owner: usize) {
        if registry::try_ipi_slots(1u64 << owner) != 0 {
            warn!(
                Task,
                "Work-debt doorbell failed for hart slot {owner}; work remains pending"
            );
        }
    }
}

pub(crate) struct Reservation<T: Send + 'static, const SLOTS: usize> {
    ledger: &'static DebtLedger<T, SLOTS>,
    reservation: Option<work_debt::Reservation>,
}

impl<T: Send + 'static, const SLOTS: usize> Reservation<T, SLOTS> {
    pub(crate) fn publish(self, value: T) {
        self.publish_inner(value, true);
    }

    pub(crate) fn publish_quiet(self, value: T) {
        self.publish_inner(value, false);
    }

    fn publish_inner(mut self, value: T, ring: bool) {
        let reservation = self
            .reservation
            .take()
            .expect("work debt reservation published twice");
        let owner = hart::current().slot();
        self.ledger.publish(reservation, owner, value, ring);
    }
}

impl<T: Send + 'static, const SLOTS: usize> Drop for Reservation<T, SLOTS> {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            self.ledger.cancel(reservation);
        }
    }
}

pub(crate) struct Taken<T: 'static, const SLOTS: usize> {
    ledger: &'static DebtLedger<T, SLOTS>,
    token: work_debt::FinishToken,
    value: T,
}

impl<T: Send + 'static, const SLOTS: usize> Taken<T, SLOTS> {
    pub(crate) fn into_parts(self) -> (Token<T, SLOTS>, T) {
        (
            Token {
                ledger: self.ledger,
                token: self.token,
            },
            self.value,
        )
    }
}

pub(crate) struct Token<T: 'static, const SLOTS: usize> {
    ledger: &'static DebtLedger<T, SLOTS>,
    token: work_debt::FinishToken,
}

impl<T: Send + 'static, const SLOTS: usize> Token<T, SLOTS> {
    pub(crate) fn requeue(self, value: T) {
        self.ledger.requeue(self.token, value);
    }

    pub(crate) fn park(self, value: T) -> work_debt::ParkResult {
        self.ledger.park(self.token, value)
    }

    pub(crate) fn finish(self) {
        self.ledger.finish(self.token);
    }

    pub(crate) fn rearm(self) -> Reservation<T, SLOTS> {
        self.ledger.rearm(self.token)
    }

    pub(crate) fn arm_wake(&self) -> Option<WakeToken<T, SLOTS>> {
        self.ledger.arm_wake(&self.token)
    }
}

pub(crate) struct WakeToken<T: 'static, const SLOTS: usize> {
    wake: work_debt::WakeToken,
    _marker: core::marker::PhantomData<&'static DebtLedger<T, SLOTS>>,
}

impl<T: Send + 'static, const SLOTS: usize> WakeToken<T, SLOTS> {
    pub(crate) fn into_raw(self) -> work_debt::WakeToken {
        self.wake
    }
}
