#![no_std]
#![forbid(unsafe_code)]

//! 固定容量、按 owner 分流的延后工作债务队列。
//!
//! 本 crate 不执行工作也不发送门铃。调用者在外部同步下于 Commit 前 Reserve，
//! 在债务成立后 Publish；owner 以固定预算 Take，并将工作 Requeue、Park 或 Finish。
//! 依赖登记前取得一次性唤醒票据，先到达的 Wake 锁存在 Taken 状态。
//! Pending 只包含可执行债务，不包含等待依赖的 Parked 债务。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveError {
    Full,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PublishError<T> {
    reservation: Reservation,
    value: T,
}

impl<T> PublishError<T> {
    pub fn into_parts(self) -> (Reservation, T) {
        (self.reservation, self.value)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RequeueError<T> {
    token: FinishToken,
    value: T,
}

impl<T> RequeueError<T> {
    pub fn into_parts(self) -> (FinishToken, T) {
        (self.token, self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Empty,
    Reserved,
    Pending,
    Taken,
    Parked,
    Retired,
}

struct Slot<T> {
    generation: u32,
    phase: Phase,
    owner: usize,
    next: Option<usize>,
    value: Option<T>,
    wake_armed: bool,
    woken: bool,
}

impl<T> Slot<T> {
    const fn empty() -> Self {
        Self {
            generation: 1,
            phase: Phase::Empty,
            owner: 0,
            next: None,
            value: None,
            wake_armed: false,
            woken: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableId(u64);

impl TableId {
    pub const fn new(value: u64) -> Self {
        assert!(value != 0);
        Self(value)
    }
}

/// 1..=9 由内核静态队列占用；运行时构造从 10 起，两个域永不碰撞。
static NEXT_TABLE_ID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(10);

/// 一组固定顺序债务在单个安全点内的公平预算。
///
/// 入口时已经 runnable 的每个后续类别各保留一个执行机会；当前类别可使用
/// 其余预算，但单个 payload 的推进仍受 `turn_limit` 限制。类别按索引单调
/// 推进，运行中才出现的工作可使用剩余预算，却不会追溯挤占早先类别。
pub struct FairBudget<const CLASSES: usize> {
    total: usize,
    turn_limit: usize,
    pending: [bool; CLASSES],
    used: usize,
    class: usize,
}

impl<const CLASSES: usize> FairBudget<CLASSES> {
    pub fn new(total: usize, turn_limit: usize, pending: [bool; CLASSES]) -> Self {
        assert!(turn_limit > 0, "fair budget turn limit must be nonzero");
        assert!(
            pending.iter().filter(|pending| **pending).count() <= total,
            "fair budget cannot reserve more classes than total work"
        );
        Self {
            total,
            turn_limit,
            pending,
            used: 0,
            class: 0,
        }
    }

    pub fn remaining(&mut self, class: usize) -> usize {
        assert!(class < CLASSES, "fair budget class is out of range");
        assert!(
            class >= self.class,
            "fair budget classes must advance monotonically"
        );
        self.class = class;
        let reserved = self.pending[class + 1..]
            .iter()
            .filter(|pending| **pending)
            .count();
        self.total
            .saturating_sub(reserved)
            .saturating_sub(self.used)
    }

    pub fn turn(&mut self, class: usize) -> usize {
        self.remaining(class).min(self.turn_limit)
    }

    pub fn charge(&mut self, class: usize, work: usize) {
        let remaining = self.remaining(class);
        assert!(
            work <= remaining,
            "fair budget charge exceeded the class allowance"
        );
        self.used += work;
    }

    pub fn used(&self) -> usize {
        self.used
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reservation {
    table_id: TableId,
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
    table_id: TableId,
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

/// 一次依赖的 affine 唤醒责任。必须 Wake 或 Cancel 后才能释放债务槽。
#[derive(Debug, PartialEq, Eq)]
pub struct WakeToken {
    table_id: TableId,
    owner: usize,
    slot: usize,
    generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkResult {
    Parked,
    /// 完成先于 Park 到达，同一债务已经重新入队。
    Runnable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeResult {
    /// Taken 的执行者尚未交回 payload；唤醒已锁存。
    Latched,
    Runnable {
        owner: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState<D> {
    Runnable,
    Blocked(D),
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepResult<D> {
    pub work_done: usize,
    pub state: StepState<D>,
}

/// `SLOTS` 个全局债务槽按 `OWNERS` 条 FIFO 链分流。Reserve 时无需预知 owner；
/// Publish 后槽只会出现在一条 owner 链中。
pub struct WorkDebts<T, const OWNERS: usize, const SLOTS: usize> {
    table_id: TableId,
    slots: [Slot<T>; SLOTS],
    heads: [Option<usize>; OWNERS],
    tails: [Option<usize>; OWNERS],
    reserve_cursor: usize,
}

impl<T, const OWNERS: usize, const SLOTS: usize> WorkDebts<T, OWNERS, SLOTS> {
    pub const fn new_with_id(table_id: TableId) -> Self {
        assert!(OWNERS > 0);
        assert!(SLOTS > 0);
        Self {
            table_id,
            slots: [const { Slot::empty() }; SLOTS],
            heads: [None; OWNERS],
            tails: [None; OWNERS],
            reserve_cursor: 0,
        }
    }

    pub fn try_new() -> Option<Self> {
        NEXT_TABLE_ID
            .allocate()
            .map(TableId::new)
            .map(Self::new_with_id)
    }

    pub fn new() -> Self {
        Self::try_new().expect("work-debt table identity exhausted")
    }

    pub const fn table_id(&self) -> TableId {
        self.table_id
    }

    pub fn reserve(&mut self) -> Result<Reservation, ReserveError> {
        self.reserve_in(0..SLOTS)
    }

    /// 在调用者定义的固定分区中准入，不向其它分区借容量。
    pub fn reserve_in(
        &mut self,
        range: core::ops::Range<usize>,
    ) -> Result<Reservation, ReserveError> {
        if range.start >= range.end || range.end > SLOTS {
            return Err(ReserveError::Full);
        }
        let cursor = if range.contains(&self.reserve_cursor) {
            self.reserve_cursor
        } else {
            range.start
        };
        let slot = (cursor..range.end)
            .chain(range.start..cursor)
            .find(|&slot| self.slots[slot].phase == Phase::Empty)
            .ok_or(ReserveError::Full)?;
        let entry = &mut self.slots[slot];
        entry.phase = Phase::Reserved;
        self.reserve_cursor = (slot + 1) % SLOTS;
        Ok(Reservation {
            table_id: self.table_id,
            slot,
            generation: entry.generation,
        })
    }

    pub fn cancel(&mut self, reservation: Reservation) -> Result<(), Reservation> {
        if reservation.table_id != self.table_id {
            return Err(reservation);
        }
        let Some(entry) = self.entry_mut(reservation.slot, reservation.generation) else {
            return Err(reservation);
        };
        if entry.phase != Phase::Reserved {
            return Err(reservation);
        }
        entry.phase = Phase::Empty;
        Ok(())
    }

    pub fn publish(
        &mut self,
        reservation: Reservation,
        owner: usize,
        value: T,
    ) -> Result<(), PublishError<T>> {
        if reservation.table_id != self.table_id || owner >= OWNERS {
            return Err(PublishError { reservation, value });
        }
        let slot = reservation.slot;
        let generation = reservation.generation;
        let Some(entry) = self.entry_mut(slot, generation) else {
            return Err(PublishError { reservation, value });
        };
        if entry.phase != Phase::Reserved {
            return Err(PublishError { reservation, value });
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
        entry.woken = false;
        let generation = entry.generation;
        let value = entry
            .value
            .take()
            .expect("pending work-debt slot must contain work");
        Some(Taken {
            token: FinishToken {
                table_id: self.table_id,
                owner,
                slot,
                generation,
            },
            value,
        })
    }

    pub fn requeue(&mut self, token: FinishToken, value: T) -> Result<(), RequeueError<T>> {
        if token.table_id != self.table_id {
            return Err(RequeueError { token, value });
        }
        let slot = token.slot;
        let generation = token.generation;
        let Some(entry) = self.entry_mut(slot, generation) else {
            return Err(RequeueError { token, value });
        };
        if entry.phase != Phase::Taken || entry.owner != token.owner || entry.wake_armed {
            return Err(RequeueError { token, value });
        }
        entry.value = Some(value);
        entry.next = None;
        entry.phase = Phase::Pending;
        self.append(token.owner, slot);
        Ok(())
    }

    /// 调用者在登记来源依赖前取得票据，并在来源同步下登记及复检条件。
    /// 同一 Taken 周期至多一个依赖；票据消费前禁止 Finish/Rearm/Requeue。
    pub fn arm_wake(&mut self, token: &FinishToken) -> Option<WakeToken> {
        if token.table_id != self.table_id {
            return None;
        }
        let entry = self.entry_mut(token.slot, token.generation)?;
        if entry.phase != Phase::Taken
            || entry.owner != token.owner
            || entry.wake_armed
            || entry.woken
        {
            return None;
        }
        entry.wake_armed = true;
        Some(WakeToken {
            table_id: token.table_id,
            owner: token.owner,
            slot: token.slot,
            generation: token.generation,
        })
    }

    pub fn cancel_wake(&mut self, wake: WakeToken) -> Result<(), WakeToken> {
        if wake.table_id != self.table_id {
            return Err(wake);
        }
        let Some(entry) = self.entry_mut(wake.slot, wake.generation) else {
            return Err(wake);
        };
        // Parked 时取消唯一唤醒会遗留必成责任；只能由执行者取消未停驻的依赖。
        if entry.phase != Phase::Taken || entry.owner != wake.owner || !entry.wake_armed {
            return Err(wake);
        }
        entry.wake_armed = false;
        Ok(())
    }

    /// 交回 payload 并等待已登记依赖；早到 Wake 则重新入队，不进入 Parked。
    pub fn park(&mut self, token: FinishToken, value: T) -> Result<ParkResult, RequeueError<T>> {
        if token.table_id != self.table_id {
            return Err(RequeueError { token, value });
        }
        let Some(entry) = self.entry_mut(token.slot, token.generation) else {
            return Err(RequeueError { token, value });
        };
        if entry.phase != Phase::Taken
            || entry.owner != token.owner
            || !(entry.wake_armed || entry.woken)
        {
            return Err(RequeueError { token, value });
        }
        entry.value = Some(value);
        entry.next = None;
        if entry.woken {
            entry.woken = false;
            entry.phase = Phase::Pending;
            self.append(token.owner, token.slot);
            Ok(ParkResult::Runnable)
        } else {
            entry.phase = Phase::Parked;
            Ok(ParkResult::Parked)
        }
    }

    pub fn wake(&mut self, wake: WakeToken) -> Result<WakeResult, WakeToken> {
        if wake.table_id != self.table_id {
            return Err(wake);
        }
        let Some(entry) = self.entry_mut(wake.slot, wake.generation) else {
            return Err(wake);
        };
        if entry.owner != wake.owner
            || !entry.wake_armed
            || !matches!(entry.phase, Phase::Taken | Phase::Parked)
        {
            return Err(wake);
        }
        entry.wake_armed = false;
        if entry.phase == Phase::Taken {
            entry.woken = true;
            Ok(WakeResult::Latched)
        } else {
            entry.phase = Phase::Pending;
            self.append(wake.owner, wake.slot);
            Ok(WakeResult::Runnable { owner: wake.owner })
        }
    }

    pub fn finish(&mut self, token: FinishToken) -> Result<(), FinishToken> {
        if token.table_id != self.table_id {
            return Err(token);
        }
        let Some(entry) = self.entry_mut(token.slot, token.generation) else {
            return Err(token);
        };
        if entry.phase != Phase::Taken || entry.owner != token.owner || entry.wake_armed {
            return Err(token);
        }
        assert!(entry.value.is_none(), "taken slot retained work at Finish");
        if entry.generation == u32::MAX {
            entry.phase = Phase::Retired;
        } else {
            entry.generation += 1;
            entry.phase = Phase::Empty;
        }
        Ok(())
    }

    /// Taken 债务完成一次交付后继续保留同一容量 owner，重新回到 Reserved。
    /// token 与 reservation 均为 affine，代次无需变化：该槽从未释放给其它准入者。
    pub fn rearm(&mut self, token: FinishToken) -> Result<Reservation, FinishToken> {
        if token.table_id != self.table_id {
            return Err(token);
        }
        let Some(entry) = self.entry_mut(token.slot, token.generation) else {
            return Err(token);
        };
        if entry.phase != Phase::Taken || entry.owner != token.owner || entry.wake_armed {
            return Err(token);
        }
        assert!(entry.value.is_none(), "taken slot retained work at Rearm");
        entry.phase = Phase::Reserved;
        Ok(Reservation {
            table_id: token.table_id,
            slot: token.slot,
            generation: token.generation,
        })
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

#[cfg(test)]
mod tests {
    use super::{ReserveError, TableId, WorkDebts};

    #[test]
    fn maximum_generation_retires_slot_permanently() {
        let mut debts: WorkDebts<(), 1, 1> = WorkDebts::new_with_id(TableId::new(99));
        debts.slots[0].generation = u32::MAX;
        let reservation = debts.reserve().unwrap();
        debts.publish(reservation, 0, ()).unwrap();
        let (token, ()) = debts.take(0).unwrap().into_parts();
        assert!(debts.finish(token).is_ok());
        assert_eq!(debts.available(), 0);
        assert_eq!(debts.reserve(), Err(ReserveError::Full));
    }
}
