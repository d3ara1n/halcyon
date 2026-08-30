#![no_std]
#![forbid(unsafe_code)]

//! 固定容量 Remote Call 请求槽的纯逻辑所有权核心。
//!
//! 本 crate 不发送 IPI、不执行远端动作，也不拥有业务完成对象。调用者在外部
//! 同步下驱动 Reserve、Publish、Take 与 Finish；generation 防止槽复用后旧
//! 身份误命中新请求。

/// 预留请求槽失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveError {
    InvalidTarget,
    Full,
}

/// Publish 时 token 已不再指向原 reservation。
#[derive(Debug, PartialEq, Eq)]
pub struct PublishError<T> {
    value: T,
}

impl<T> PublishError<T> {
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
    value: Option<T>,
}

impl<T> Slot<T> {
    const fn empty() -> Self {
        Self {
            generation: 1,
            phase: Phase::Empty,
            value: None,
        }
    }
}

/// 一项尚未发布的 affine reservation。
#[derive(Debug, PartialEq, Eq)]
pub struct Reservation {
    target: usize,
    slot: usize,
    generation: u32,
}

impl Reservation {
    pub const fn target(&self) -> usize {
        self.target
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

/// 已从 Pending 槽取出的请求。动作完成后必须把 finish token 交回表。
pub struct Taken<T> {
    token: FinishToken,
    value: T,
}

impl<T> Taken<T> {
    pub fn into_parts(self) -> (FinishToken, T) {
        (self.token, self.value)
    }
}

/// Taken 槽的 affine 完成权。
#[derive(Debug, PartialEq, Eq)]
pub struct FinishToken {
    target: usize,
    slot: usize,
    generation: u32,
}

impl FinishToken {
    pub const fn target(&self) -> usize {
        self.target
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

/// 每个目标 hart 固定 `SLOTS` 项的请求表。
pub struct RemoteCalls<T, const HARTS: usize, const SLOTS: usize> {
    slots: [[Slot<T>; SLOTS]; HARTS],
}

impl<T, const HARTS: usize, const SLOTS: usize> RemoteCalls<T, HARTS, SLOTS> {
    pub const fn new() -> Self {
        assert!(HARTS > 0);
        assert!(SLOTS > 0);
        Self {
            slots: [const { [const { Slot::empty() }; SLOTS] }; HARTS],
        }
    }

    /// 预留目标 hart 的一个槽。失败不改变任何槽。
    pub fn reserve(&mut self, target: usize) -> Result<Reservation, ReserveError> {
        let row = self
            .slots
            .get_mut(target)
            .ok_or(ReserveError::InvalidTarget)?;
        let (slot, entry) = row
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.phase == Phase::Empty)
            .ok_or(ReserveError::Full)?;
        entry.phase = Phase::Reserved;
        Ok(Reservation {
            target,
            slot,
            generation: entry.generation,
        })
    }

    /// Commit 前取消 reservation。陈旧或错配 token 返回 false。
    pub fn cancel(&mut self, reservation: Reservation) -> bool {
        let Some(entry) =
            self.entry_mut(reservation.target, reservation.slot, reservation.generation)
        else {
            return false;
        };
        if entry.phase != Phase::Reserved {
            return false;
        }
        entry.phase = Phase::Empty;
        true
    }

    /// 发布请求。成功后请求只能由目标 hart 取得，不能取消。
    pub fn publish(&mut self, reservation: Reservation, value: T) -> Result<(), PublishError<T>> {
        let Some(entry) =
            self.entry_mut(reservation.target, reservation.slot, reservation.generation)
        else {
            return Err(PublishError { value });
        };
        if entry.phase != Phase::Reserved {
            return Err(PublishError { value });
        }
        debug_assert!(entry.value.is_none());
        entry.value = Some(value);
        entry.phase = Phase::Pending;
        Ok(())
    }

    /// 目标 hart 取得一项 Pending 请求。调用者必须有界地调用本方法；新请求可
    /// 在旧请求 Finish 后复用槽，因此不提供“排空至空”的无界保证。
    pub fn take(&mut self, target: usize) -> Option<Taken<T>> {
        let row = self.slots.get_mut(target)?;
        let (slot, entry) = row
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.phase == Phase::Pending)?;
        entry.phase = Phase::Taken;
        let value = entry
            .value
            .take()
            .expect("pending remote-call slot must contain a request");
        Some(Taken {
            token: FinishToken {
                target,
                slot,
                generation: entry.generation,
            },
            value,
        })
    }

    /// 发布动作完成并回收槽。generation 耗尽时槽永久 Retired，避免 ABA。
    pub fn finish(&mut self, token: FinishToken) -> bool {
        let Some(entry) = self.entry_mut(token.target, token.slot, token.generation) else {
            return false;
        };
        if entry.phase != Phase::Taken {
            return false;
        }
        debug_assert!(entry.value.is_none());
        if entry.generation == u32::MAX {
            entry.phase = Phase::Retired;
        } else {
            entry.generation += 1;
            entry.phase = Phase::Empty;
        }
        true
    }

    pub fn has_pending(&self, target: usize) -> bool {
        self.slots
            .get(target)
            .is_some_and(|row| row.iter().any(|entry| entry.phase == Phase::Pending))
    }

    pub fn available(&self, target: usize) -> Option<usize> {
        self.slots.get(target).map(|row| {
            row.iter()
                .filter(|entry| entry.phase == Phase::Empty)
                .count()
        })
    }

    fn entry_mut(&mut self, target: usize, slot: usize, generation: u32) -> Option<&mut Slot<T>> {
        let entry = self.slots.get_mut(target)?.get_mut(slot)?;
        (entry.generation == generation).then_some(entry)
    }
}

impl<T, const HARTS: usize, const SLOTS: usize> Default for RemoteCalls<T, HARTS, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}
