#![no_std]
#![forbid(unsafe_code)]

//! 固定容量、按 owner 分流的延后工作债务队列。
//!
//! 本 crate 不执行工作也不发送门铃。调用者在外部同步下于 Commit 前 Reserve，
//! 在债务成立后 Publish；owner 以固定预算 Take，并将未完成工作 Requeue 或 Finish。
//! Pending 电平而非门铃边沿是真值。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveError {
    Full,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PublishError<T> {
    value: T,
}

impl<T> PublishError<T> {
    pub fn into_value(self) -> T {
        self.value
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RequeueError<T> {
    value: T,
}

impl<T> RequeueError<T> {
    pub fn into_value(self) -> T {
        self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Empty,
    Reserved,
    Pending,
    Taken,
    Retired,
}

struct Slot<T> {
    generation: u32,
    phase: Phase,
    owner: usize,
    next: Option<usize>,
    value: Option<T>,
}

impl<T> Slot<T> {
    const fn empty() -> Self {
        Self {
            generation: 1,
            phase: Phase::Empty,
            owner: 0,
            next: None,
            value: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reservation {
    slot: usize,
    generation: u32,
}

impl Reservation {
    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

pub struct Taken<T> {
    token: FinishToken,
    value: T,
}

impl<T> Taken<T> {
    pub fn into_parts(self) -> (FinishToken, T) {
        (self.token, self.value)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FinishToken {
    owner: usize,
    slot: usize,
    generation: u32,
}

impl FinishToken {
    pub const fn owner(&self) -> usize {
        self.owner
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

/// `SLOTS` 个全局债务槽按 `OWNERS` 条 FIFO 链分流。Reserve 时无需预知 owner；
/// Publish 后槽只会出现在一条 owner 链中。
pub struct WorkDebts<T, const OWNERS: usize, const SLOTS: usize> {
    slots: [Slot<T>; SLOTS],
    heads: [Option<usize>; OWNERS],
    tails: [Option<usize>; OWNERS],
}

impl<T, const OWNERS: usize, const SLOTS: usize> WorkDebts<T, OWNERS, SLOTS> {
    pub const fn new() -> Self {
        assert!(OWNERS > 0);
        assert!(SLOTS > 0);
        Self {
            slots: [const { Slot::empty() }; SLOTS],
            heads: [None; OWNERS],
            tails: [None; OWNERS],
        }
    }

    pub fn reserve(&mut self) -> Result<Reservation, ReserveError> {
        let (slot, entry) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.phase == Phase::Empty)
            .ok_or(ReserveError::Full)?;
        entry.phase = Phase::Reserved;
        Ok(Reservation {
            slot,
            generation: entry.generation,
        })
    }

    pub fn cancel(&mut self, reservation: Reservation) -> bool {
        let Some(entry) = self.entry_mut(reservation.slot, reservation.generation) else {
            return false;
        };
        if entry.phase != Phase::Reserved {
            return false;
        }
        entry.phase = Phase::Empty;
        true
    }

    pub fn publish(
        &mut self,
        reservation: Reservation,
        owner: usize,
        value: T,
    ) -> Result<(), PublishError<T>> {
        if owner >= OWNERS {
            return Err(PublishError { value });
        }
        let slot = reservation.slot;
        let Some(entry) = self.entry_mut(slot, reservation.generation) else {
            return Err(PublishError { value });
        };
        if entry.phase != Phase::Reserved {
            return Err(PublishError { value });
        }
        entry.owner = owner;
        entry.next = None;
        entry.value = Some(value);
        entry.phase = Phase::Pending;
        self.append(owner, slot);
        Ok(())
    }

    pub fn take(&mut self, owner: usize) -> Option<Taken<T>> {
        let head = *self.heads.get(owner)?;
        let slot = head?;
        let entry = &mut self.slots[slot];
        assert_eq!(
            entry.phase,
            Phase::Pending,
            "owner queue linked a non-pending slot"
        );
        assert_eq!(entry.owner, owner, "owner queue linked a foreign slot");
        let next = entry.next.take();
        self.heads[owner] = next;
        if next.is_none() {
            self.tails[owner] = None;
        }
        entry.phase = Phase::Taken;
        let generation = entry.generation;
        let value = entry
            .value
            .take()
            .expect("pending work-debt slot must contain work");
        Some(Taken {
            token: FinishToken {
                owner,
                slot,
                generation,
            },
            value,
        })
    }

    pub fn requeue(&mut self, token: FinishToken, value: T) -> Result<(), RequeueError<T>> {
        let slot = token.slot;
        let Some(entry) = self.entry_mut(slot, token.generation) else {
            return Err(RequeueError { value });
        };
        if entry.phase != Phase::Taken || entry.owner != token.owner {
            return Err(RequeueError { value });
        }
        entry.value = Some(value);
        entry.next = None;
        entry.phase = Phase::Pending;
        self.append(token.owner, slot);
        Ok(())
    }

    pub fn finish(&mut self, token: FinishToken) -> bool {
        let Some(entry) = self.entry_mut(token.slot, token.generation) else {
            return false;
        };
        if entry.phase != Phase::Taken || entry.owner != token.owner {
            return false;
        }
        assert!(entry.value.is_none(), "taken slot retained work at Finish");
        if entry.generation == u32::MAX {
            entry.phase = Phase::Retired;
        } else {
            entry.generation += 1;
            entry.phase = Phase::Empty;
        }
        true
    }

    pub fn has_pending(&self, owner: usize) -> bool {
        self.heads.get(owner).is_some_and(Option::is_some)
    }

    pub fn available(&self) -> usize {
        self.slots
            .iter()
            .filter(|entry| entry.phase == Phase::Empty)
            .count()
    }

    fn append(&mut self, owner: usize, slot: usize) {
        match self.tails[owner].replace(slot) {
            Some(tail) => {
                assert!(self.slots[tail].next.replace(slot).is_none());
            }
            None => {
                assert!(self.heads[owner].replace(slot).is_none());
            }
        }
    }

    fn entry_mut(&mut self, slot: usize, generation: u32) -> Option<&mut Slot<T>> {
        let entry = self.slots.get_mut(slot)?;
        (entry.generation == generation).then_some(entry)
    }
}

impl<T, const OWNERS: usize, const SLOTS: usize> Default for WorkDebts<T, OWNERS, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}
