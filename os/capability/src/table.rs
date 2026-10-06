//! 进程本地 capability 表。
//!
//! 槽状态：Vacant / Occupied / Reserved（不可见的安装预留）/ Claimed（操作已
//! 接管 owner 的返还预约）/ Retired（代次耗尽）。调用方负责外层互斥。
//!
//! 返还预约契约：[`CapTable::claim`] 在线性化点把 [`Cap`] 交给操作，原槽只
//! 保留返还资格。Close 撤销该资格而不取消操作；操作失败时
//! [`CapTable::restore`] 仅在资格仍有效且表未关闭时恢复原槽，否则释放。
//! 仲裁依据预约身份与槽代次，不依据槽下标。

use erhino_shared::object::{Handle, Rights};
use funded_storage::{Buffer, Funding, StorageError};

use crate::{Authority, HoldersError};

/// Handle 的 ABI 槽位宽度；实际保留容量同时受付款额度与分配约束。
pub const DEFAULT_HANDLE_LIMIT: usize = u32::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    InvalidHandle,
    StaleHandle,
    /// 槽已被在途操作接管；只有 Close 可作用于它。
    Claimed,
    RightsDenied,
    DuplicateHandle,
    ReachLimit,
    BadReservation,
    AllocationFailed,
    FundingDenied,
    /// 表已关闭（进程退出），不再接受安装或返还。
    Closed,
    /// 对象已终结，不能再铸造 authority。
    Sealed,
}

impl From<HoldersError> for TableError {
    fn from(error: HoldersError) -> Self {
        match error {
            HoldersError::Sealed => Self::Sealed,
            HoldersError::ReachLimit => Self::ReachLimit,
        }
    }
}

fn storage_error<E>(error: StorageError<E>) -> TableError {
    match error {
        StorageError::Funding(_) => TableError::FundingDenied,
        StorageError::Allocation => TableError::AllocationFailed,
        StorageError::Layout => TableError::ReachLimit,
    }
}

/// Terminal 对象最后一份 capability 已消失；由内核在锁外交给对象执行。
#[must_use = "last-capability events must be delivered to the object"]
#[derive(Debug)]
pub struct LastCapability<A>(pub A);

/// 一份 authority：对象存储引用 + rights。不实现 Clone；复制只经
/// [`Cap::derive`]，放弃只经 [`Cap::release`]。
#[derive(Debug)]
pub struct Cap<A: Authority> {
    object: Option<A>,
    rights: Rights,
}

impl<A: Authority> Cap<A> {
    /// 由对象操作铸造一份新 authority。rights 必须是已知位。
    pub fn mint(object: A, rights: Rights) -> Result<Self, TableError> {
        if !rights.is_known() {
            return Err(TableError::RightsDenied);
        }
        object.holders().acquire()?;
        Ok(Self {
            object: Some(object),
            rights,
        })
    }

    pub fn object(&self) -> &A {
        self.object.as_ref().expect("capability already released")
    }

    pub const fn rights(&self) -> Rights {
        self.rights
    }

    /// 受权复制：源需 DUPLICATE，派生 rights 为源子集。
    pub fn derive(&self, rights: Rights) -> Result<Self, TableError> {
        if !rights.is_known() || !self.rights.contains(Rights::DUPLICATE) {
            return Err(TableError::RightsDenied);
        }
        if !rights.is_subset_of(self.rights) {
            return Err(TableError::RightsDenied);
        }
        Self::mint(self.object().clone(), rights)
    }

    /// 移交时裁剪 rights：只能收窄，不改变计数。
    pub fn restrict(mut self, rights: Rights) -> Result<Self, (Self, TableError)> {
        if let Err(error) = self.attenuate(rights) {
            return Err((self, error));
        }
        Ok(self)
    }

    /// 已接管事务在业务提交后原位裁剪，不重铸 authority 或分配新容器。
    pub fn attenuate(&mut self, rights: Rights) -> Result<(), TableError> {
        if !rights.is_known() || !rights.is_subset_of(self.rights) {
            return Err(TableError::RightsDenied);
        }
        self.rights = rights;
        Ok(())
    }

    /// 放弃这份 authority。表内关闭、在途丢弃、事务释放与进程回收都经此处。
    pub fn release(mut self) -> Option<LastCapability<A>> {
        let object = self.object.take().expect("capability already released");
        object.holders().release().then_some(LastCapability(object))
    }
}

impl<A: Authority> Drop for Cap<A> {
    fn drop(&mut self) {
        // 审计探针：authority 必须显式 release 或移交。
        debug_assert!(self.object.is_none(), "capability dropped without release");
    }
}

/// 返还预约身份。只能由产生它的表消费一次。
#[must_use = "a claim must be committed or restored"]
#[derive(Debug, PartialEq, Eq)]
pub struct ClaimTicket {
    handle: Handle,
    id: u64,
}

impl ClaimTicket {
    pub const fn handle(&self) -> Handle {
        self.handle
    }
}

/// [`CapTable::restore`] 的结果。
#[derive(Debug)]
pub enum Restore<A: Authority> {
    /// 预约有效，原 handle 重新可用。
    Restored,
    /// 预约已被 Close 撤销或表已关闭，authority 已释放。
    Released(Option<LastCapability<A>>),
}

/// [`CapTable::take_next_bounded`] 的结果。
pub enum TakeNext<A: Authority> {
    Cap(Cap<A>),
    /// 预算耗尽且未到表尾。
    Progress,
    Exhausted,
}

/// 一批尚未对进程可见的槽位。
#[must_use = "a reservation must be committed or rolled back"]
#[derive(Debug)]
pub struct Reservation<A: Authority, C> {
    token: u64,
    handles: Buffer<Handle, C>,
    releases: Buffer<LastCapability<A>, C>,
}

impl<A: Authority, C> Reservation<A, C> {
    pub fn handles(&self) -> &[Handle] {
        &self.handles
    }
}

/// 对象已提交后，返还资格丢失只能交出释放事件，不能推翻对象提交。
#[derive(Debug)]
pub enum Commit<A: Authority, C> {
    Installed(Buffer<Handle, C>),
    Released {
        handles: Buffer<Handle, C>,
        events: Buffer<LastCapability<A>, C>,
    },
}

#[derive(Debug)]
enum SlotState<A: Authority> {
    Vacant,
    Occupied(Cap<A>),
    Reserved(u64),
    Claimed(u64),
    Retired,
}

#[derive(Debug)]
struct Slot<A: Authority> {
    generation: u32,
    state: SlotState<A>,
}

#[derive(Debug)]
pub struct CapTable<A: Authority, C> {
    slots: Buffer<Slot<A>, C>,
    limit: usize,
    occupied: usize,
    next_token: u64,
    closed: bool,
}

impl<A: Authority, C> Drop for CapTable<A, C> {
    fn drop(&mut self) {
        if self.occupied == 0 {
            // 计数包含 Occupied/Claimed；归零证明没有槽内 authority。
            // 剩余状态只有整数，不重跑已由 bounded 清理履行的责任。
            self.slots.forget_values();
        }
    }
}

impl<A: Authority, C> CapTable<A, C> {
    pub fn try_new<F: Funding<Charge = C>>(payer: &F) -> Result<Self, TableError> {
        Self::try_with_limit(DEFAULT_HANDLE_LIMIT, payer)
    }

    pub fn try_with_limit<F: Funding<Charge = C>>(
        limit: usize,
        payer: &F,
    ) -> Result<Self, TableError> {
        let mut slots = Buffer::try_empty(1, payer).map_err(storage_error)?;
        // slot 0 永远无效。
        slots.push(Slot {
            generation: 0,
            state: SlotState::Retired,
        });
        Ok(Self {
            slots,
            limit: limit.min(u32::MAX as usize),
            occupied: 0,
            next_token: 1,
            closed: false,
        })
    }

    pub const fn len(&self) -> usize {
        self.occupied
    }

    pub const fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// 校验 rights 并借用 capability。
    pub fn get(&self, handle: Handle, required: Rights) -> Result<&Cap<A>, TableError> {
        if self.closed {
            return Err(TableError::Closed);
        }
        let index = self.index(handle)?;
        match &self.slots[index].state {
            SlotState::Occupied(cap) if cap.rights.contains(required) => Ok(cap),
            SlotState::Occupied(_) => Err(TableError::RightsDenied),
            SlotState::Claimed(_) => Err(TableError::Claimed),
            _ => Err(TableError::StaleHandle),
        }
    }

    /// 立即安装。失败时返还 owner。
    pub fn insert<F: Funding<Charge = C>>(
        &mut self,
        cap: Cap<A>,
        payer: &F,
    ) -> Result<Handle, (Cap<A>, TableError)> {
        if self.closed {
            return Err((cap, TableError::Closed));
        }
        match self.reserve_slot(0, payer) {
            Ok(handle) => {
                self.slots[handle.slot() as usize].state = SlotState::Occupied(cap);
                self.occupied += 1;
                Ok(handle)
            }
            Err(error) => Err((cap, error)),
        }
    }

    /// Close：合法 owner 必成的本地释放。对已接管槽只撤销返还资格，
    /// 返回 None（authority 由操作持有）；对普通槽摘出 capability 交调用方
    /// 在锁外 release。
    pub fn close(&mut self, handle: Handle) -> Result<Option<Cap<A>>, TableError> {
        let index = self.index(handle)?;
        let taken = match core::mem::replace(&mut self.slots[index].state, SlotState::Vacant) {
            SlotState::Occupied(cap) => Some(cap),
            SlotState::Claimed(_) => None,
            other => {
                self.slots[index].state = other;
                return Err(TableError::StaleHandle);
            }
        };
        self.occupied -= 1;
        self.advance_generation(index);
        Ok(taken)
    }

    /// 操作在线性化点接管 owner；原槽保留返还预约。
    pub fn claim(
        &mut self,
        handle: Handle,
        required: Rights,
    ) -> Result<(Cap<A>, ClaimTicket), TableError> {
        self.get(handle, required)?;
        let id = self.token()?;
        let index = handle.slot() as usize;
        let SlotState::Occupied(cap) =
            core::mem::replace(&mut self.slots[index].state, SlotState::Claimed(id))
        else {
            unreachable!("validated slot must be occupied")
        };
        Ok((cap, ClaimTicket { handle, id }))
    }

    /// 操作成功：撤掉尚存的预约。authority 已由操作消费或移交。
    pub fn commit_claim(&mut self, ticket: ClaimTicket) {
        if let Some(index) = self.claimed_index(&ticket) {
            self.slots[index].state = SlotState::Vacant;
            self.occupied -= 1;
            self.advance_generation(index);
        }
    }

    /// 操作失败：预约有效且表未关闭则原槽恢复，否则释放 authority。
    pub fn restore(&mut self, ticket: ClaimTicket, cap: Cap<A>) -> Restore<A> {
        match self.claimed_index(&ticket) {
            Some(index) if !self.closed => {
                self.slots[index].state = SlotState::Occupied(cap);
                Restore::Restored
            }
            _ => Restore::Released(cap.release()),
        }
    }

    /// 关闭表：此后不接受安装与返还。线性化点即此调用。
    pub fn shut(&mut self) {
        self.closed = true;
    }

    /// 预留一批不可见槽位；失败不留下部分预留。
    pub fn reserve<F: Funding<Charge = C>>(
        &mut self,
        count: usize,
        payer: &F,
    ) -> Result<Reservation<A, C>, TableError> {
        if self.closed {
            return Err(TableError::Closed);
        }
        let token = self.token()?;
        let mut handles = Buffer::try_empty(count, payer).map_err(storage_error)?;
        let releases = Buffer::try_empty(count, payer).map_err(storage_error)?;
        for _ in 0..count {
            match self.reserve_slot(token, payer) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    self.rollback_handles(&handles, token);
                    return Err(error);
                }
            }
        }
        Ok(Reservation {
            token,
            handles,
            releases,
        })
    }

    /// 公开预留槽；shut 后使用预约时预备的空间返还释放事件，提交不分配。
    /// 数量/预约结构错误才返回 Err；shut 是正常的返还资格撤销。
    pub fn commit<I: IntoIterator<Item = Cap<A>>>(
        &mut self,
        mut reservation: Reservation<A, C>,
        caps: I,
    ) -> Result<Commit<A, C>, (I::IntoIter, TableError)>
    where
        I::IntoIter: ExactSizeIterator,
    {
        let caps = caps.into_iter();
        if reservation.handles.len() != caps.len() {
            self.rollback_handles(&reservation.handles, reservation.token);
            return Err((caps, TableError::BadReservation));
        }
        if self.closed {
            self.rollback_handles(&reservation.handles, reservation.token);
            for cap in caps {
                if let Some(event) = cap.release() {
                    reservation.releases.push(event);
                }
            }
            return Ok(Commit::Released {
                handles: reservation.handles,
                events: reservation.releases,
            });
        }
        let valid = reservation
            .handles
            .iter()
            .all(|h| self.is_reserved(*h, reservation.token));
        if !valid {
            self.rollback_handles(&reservation.handles, reservation.token);
            return Err((caps, TableError::BadReservation));
        }
        for (handle, cap) in reservation.handles.iter().zip(caps) {
            self.slots[handle.slot() as usize].state = SlotState::Occupied(cap);
            self.occupied += 1;
        }
        Ok(Commit::Installed(reservation.handles))
    }

    /// 撤销预留；代次前进，暂存数值永不复活。
    pub fn rollback(&mut self, reservation: Reservation<A, C>) {
        self.rollback_handles(&reservation.handles, reservation.token);
    }

    /// 原子校验并摘出一批待移交的 capability（源需 `transfer` 位，rights 收窄）。
    /// 任一校验失败表保持不变。`transfer` 由调用方根据运输/装配操作传入。
    pub fn extract<F: Funding<Charge = C>>(
        &mut self,
        items: &[(Handle, Rights)],
        transfer: Rights,
        payer: &F,
    ) -> Result<Buffer<Cap<A>, C>, TableError> {
        let mut out = Buffer::try_empty(items.len(), payer).map_err(storage_error)?;
        for (i, (handle, rights)) in items.iter().enumerate() {
            if items[..i].iter().any(|(prior, _)| prior == handle) {
                return Err(TableError::DuplicateHandle);
            }
            let cap = self.get(*handle, transfer)?;
            if !rights.is_known() || !rights.is_subset_of(cap.rights) {
                return Err(TableError::RightsDenied);
            }
        }
        for (handle, rights) in items {
            let cap = self
                .close(*handle)
                .expect("validated handle remains installed")
                .expect("validated handle is occupied");
            let Ok(cap) = cap.restrict(*rights) else {
                unreachable!("validated rights are a subset")
            };
            out.push(cap);
        }
        Ok(out)
    }

    /// 有界摘出：至多扫描 `max_slots` 个槽。Claimed 槽撤销返还资格，
    /// Reserved 槽回收；调用方应先 [`shut`](Self::shut)。
    pub fn take_next_bounded(&mut self, cursor: &mut usize, max_slots: usize) -> TakeNext<A> {
        let mut scanned = 0;
        while *cursor < self.slots.len() {
            if scanned >= max_slots {
                return TakeNext::Progress;
            }
            let index = *cursor;
            *cursor += 1;
            scanned += 1;
            match core::mem::replace(&mut self.slots[index].state, SlotState::Vacant) {
                SlotState::Occupied(cap) => {
                    self.occupied -= 1;
                    self.advance_generation(index);
                    return TakeNext::Cap(cap);
                }
                SlotState::Claimed(_) => {
                    self.occupied -= 1;
                    self.advance_generation(index);
                }
                SlotState::Reserved(_) => self.advance_generation(index),
                other => self.slots[index].state = other,
            }
        }
        TakeNext::Exhausted
    }

    fn token(&mut self) -> Result<u64, TableError> {
        let token = self.next_token;
        self.next_token = token.checked_add(1).ok_or(TableError::ReachLimit)?;
        Ok(token)
    }

    fn index(&self, handle: Handle) -> Result<usize, TableError> {
        if !handle.is_valid() {
            return Err(TableError::InvalidHandle);
        }
        let index = handle.slot() as usize;
        match self.slots.get(index) {
            Some(slot) if slot.generation == handle.generation() => Ok(index),
            _ => Err(TableError::StaleHandle),
        }
    }

    fn claimed_index(&self, ticket: &ClaimTicket) -> Option<usize> {
        let index = self.index(ticket.handle).ok()?;
        matches!(self.slots[index].state, SlotState::Claimed(id) if id == ticket.id)
            .then_some(index)
    }

    /// 空槽线性扫描（已知简化）：满表前每次 O(n)；真实负载出现时改为空闲链。
    fn reserve_slot<F: Funding<Charge = C>>(
        &mut self,
        token: u64,
        payer: &F,
    ) -> Result<Handle, TableError> {
        if let Some(index) =
            (1..self.slots.len()).find(|&i| matches!(self.slots[i].state, SlotState::Vacant))
        {
            let slot = &mut self.slots[index];
            slot.state = SlotState::Reserved(token);
            return Ok(Handle::from_parts(index as u32, slot.generation));
        }
        if self.slots.len() > self.limit {
            return Err(TableError::ReachLimit);
        }
        if self.slots.len() == self.slots.capacity() {
            let target = self
                .slots
                .capacity()
                .saturating_mul(2)
                .max(self.slots.len() + 1)
                .min(self.limit.saturating_add(1));
            let mut prepared = Buffer::try_empty(target, payer).map_err(storage_error)?;
            self.slots
                .replace_storage(&mut prepared)
                .expect("prepared handle slots fit current data");
            // 已摘出的旧空存储完整释放；外层 HANDLE_TABLE 的秩允许短 Pool 退款边。
            drop(prepared);
        }
        let index = self.slots.len();
        self.slots.push(Slot {
            generation: 1,
            state: SlotState::Reserved(token),
        });
        Ok(Handle::from_parts(index as u32, 1))
    }

    fn is_reserved(&self, handle: Handle, token: u64) -> bool {
        self.index(handle)
            .is_ok_and(|i| matches!(self.slots[i].state, SlotState::Reserved(t) if t == token))
    }

    fn rollback_handles(&mut self, handles: &[Handle], token: u64) {
        for handle in handles {
            if self.is_reserved(*handle, token) {
                self.advance_generation(handle.slot() as usize);
            }
        }
    }

    fn advance_generation(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        if slot.generation == u32::MAX {
            slot.state = SlotState::Retired;
        } else {
            slot.generation += 1;
            slot.state = SlotState::Vacant;
        }
    }
}

#[cfg(test)]
mod tests;
