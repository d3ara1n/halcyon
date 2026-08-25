#![no_std]

//! 进程本地 Handle 表的纯逻辑实现。
//!
//! 本 crate 只管理槽位代际、rights、预留、move/duplicate 与 drain；
//! 对象类型和 lifecycle role 的语义由内核包装层提供。

extern crate alloc;

use alloc::vec::Vec;
use erhino_shared::object::{Handle, Rights};

/// 单进程首期 Handle 槽位上限。
pub const DEFAULT_HANDLE_LIMIT: usize = 65_536;

/// Handle 表操作错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    InvalidHandle,
    StaleHandle,
    RightsDenied,
    DuplicateHandle,
    ReachLimit,
    BadReservation,
    AllocationFailed,
}

/// 一个已安装或在途的对象引用。
#[derive(Debug)]
pub struct Entry<T, R> {
    object: T,
    role: R,
    rights: Rights,
}

impl<T, R> Entry<T, R> {
    pub const fn new(object: T, role: R, rights: Rights) -> Self {
        Self {
            object,
            role,
            rights,
        }
    }

    pub fn object(&self) -> &T {
        &self.object
    }

    pub fn role(&self) -> &R {
        &self.role
    }

    pub const fn rights(&self) -> Rights {
        self.rights
    }

    pub fn into_parts(self) -> (T, R, Rights) {
        (self.object, self.role, self.rights)
    }

    fn with_rights(mut self, rights: Rights) -> Self {
        self.rights = rights;
        self
    }
}

impl<T: Clone, R: Clone> Clone for Entry<T, R> {
    fn clone(&self) -> Self {
        Self {
            object: self.object.clone(),
            role: self.role.clone(),
            rights: self.rights,
        }
    }
}

#[derive(Debug)]
enum SlotState<T, R> {
    Vacant,
    Occupied(Entry<T, R>),
    Reserved(u64),
    Retired,
}

#[derive(Debug)]
struct Slot<T, R> {
    generation: u32,
    state: SlotState<T, R>,
}

/// 一批尚未对进程可见的 Handle 槽位。
#[derive(Debug)]
pub struct Reservation {
    token: u64,
    handles: Vec<Handle>,
}

impl Reservation {
    pub fn handles(&self) -> &[Handle] {
        &self.handles
    }

    pub const fn token(&self) -> u64 {
        self.token
    }
}

/// 进程本地 Handle 表。调用方负责外层并发互斥。
#[derive(Debug)]
pub struct HandleTable<T, R> {
    slots: Vec<Slot<T, R>>,
    limit: usize,
    occupied: usize,
}

impl<T, R> HandleTable<T, R> {
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_HANDLE_LIMIT)
    }

    pub fn with_limit(limit: usize) -> Self {
        let mut slots = Vec::new();
        // slot 0 永远无效，结构上直接退休。
        slots.push(Slot {
            generation: 0,
            state: SlotState::Retired,
        });
        Self {
            slots,
            limit: limit.min(u32::MAX as usize),
            occupied: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.occupied
    }

    pub const fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    /// 安装一个立即可见的 entry。
    pub fn insert(&mut self, entry: Entry<T, R>) -> Result<Handle, TableError> {
        let handle = self.reserve_slot(0)?;
        let index = handle.slot() as usize;
        self.slots[index].state = SlotState::Occupied(entry);
        self.occupied += 1;
        Ok(handle)
    }

    pub fn get(&self, handle: Handle, required: Rights) -> Result<&Entry<T, R>, TableError> {
        let index = self.occupied_index(handle)?;
        let SlotState::Occupied(entry) = &self.slots[index].state else {
            unreachable!("occupied_index only returns occupied slots")
        };
        if !entry.rights.contains(required) {
            return Err(TableError::RightsDenied);
        }
        Ok(entry)
    }

    /// 移除一个 entry。调用方必须在释放表锁后处理其 lifecycle role。
    pub fn remove(&mut self, handle: Handle) -> Result<Entry<T, R>, TableError> {
        let index = self.occupied_index(handle)?;
        let state = core::mem::replace(&mut self.slots[index].state, SlotState::Vacant);
        let SlotState::Occupied(entry) = state else {
            unreachable!()
        };
        self.occupied -= 1;
        self.advance_generation(index);
        Ok(entry)
    }

    /// 取得允许 DUPLICATE 的派生 entry，但尚不安装到表中。
    pub fn derive(&self, source: Handle, rights: Rights) -> Result<Entry<T, R>, TableError>
    where
        T: Clone,
        R: Clone,
    {
        if !rights.is_known() {
            return Err(TableError::RightsDenied);
        }
        let entry = self.get(source, Rights::DUPLICATE)?;
        if !rights.is_subset_of(entry.rights) {
            return Err(TableError::RightsDenied);
        }
        Ok(entry.clone().with_rights(rights))
    }

    /// 复制一个允许 DUPLICATE 的 entry，并裁剪 rights。
    pub fn duplicate(&mut self, source: Handle, rights: Rights) -> Result<Handle, TableError>
    where
        T: Clone,
        R: Clone,
    {
        let derived = self.derive(source, rights)?;
        self.insert(derived)
    }

    /// 原子验证并移除一批待转移 Handle。任何验证失败都保持表不变。
    pub fn extract_moves(
        &mut self,
        moves: &[(Handle, Rights)],
    ) -> Result<Vec<Entry<T, R>>, TableError> {
        let mut extracted = Vec::new();
        extracted
            .try_reserve(moves.len())
            .map_err(|_| TableError::AllocationFailed)?;

        for (i, (handle, rights)) in moves.iter().copied().enumerate() {
            if moves[..i].iter().any(|(prior, _)| *prior == handle) {
                return Err(TableError::DuplicateHandle);
            }
            if !rights.is_known() {
                return Err(TableError::RightsDenied);
            }
            let entry = self.get(handle, Rights::TRANSFER)?;
            if !rights.is_subset_of(entry.rights) {
                return Err(TableError::RightsDenied);
            }
        }

        for (handle, rights) in moves.iter().copied() {
            extracted.push(self.remove(handle)?.with_rights(rights));
        }
        Ok(extracted)
    }

    /// 预留一批不可见槽位；失败时不会留下部分预留。
    pub fn reserve(&mut self, count: usize, token: u64) -> Result<Reservation, TableError> {
        if token == 0
            || self
                .slots
                .iter()
                .any(|s| matches!(s.state, SlotState::Reserved(t) if t == token))
        {
            return Err(TableError::BadReservation);
        }
        let mut handles = Vec::new();
        handles
            .try_reserve(count)
            .map_err(|_| TableError::AllocationFailed)?;
        for _ in 0..count {
            match self.reserve_slot(token) {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    self.rollback_handles(&handles, token);
                    return Err(error);
                }
            }
        }
        Ok(Reservation { token, handles })
    }

    /// 把预留槽位一次性提交为可见 entries。
    pub fn commit(
        &mut self,
        reservation: Reservation,
        entries: Vec<Entry<T, R>>,
    ) -> Result<Vec<Handle>, TableError> {
        if reservation.handles.len() != entries.len()
            || !reservation
                .handles
                .iter()
                .all(|handle| self.is_reserved(*handle, reservation.token))
        {
            self.rollback_handles(&reservation.handles, reservation.token);
            return Err(TableError::BadReservation);
        }
        for (handle, entry) in reservation.handles.iter().copied().zip(entries) {
            self.slots[handle.slot() as usize].state = SlotState::Occupied(entry);
            self.occupied += 1;
        }
        Ok(reservation.handles)
    }

    /// 撤销预留；generation 同步前进，使失败输出中的暂存数值永不复活。
    pub fn rollback(&mut self, reservation: Reservation) -> Result<(), TableError> {
        if !reservation
            .handles
            .iter()
            .all(|handle| self.is_reserved(*handle, reservation.token))
        {
            return Err(TableError::BadReservation);
        }
        self.rollback_handles(&reservation.handles, reservation.token);
        Ok(())
    }

    /// 从 cursor 起摘出下一项；供不能移动整张表的零分配退出路径使用。
    pub fn take_next(&mut self, cursor: &mut usize) -> Option<Entry<T, R>> {
        while *cursor < self.slots.len() {
            let index = *cursor;
            *cursor += 1;
            if matches!(self.slots[index].state, SlotState::Occupied(_)) {
                let state = core::mem::replace(&mut self.slots[index].state, SlotState::Vacant);
                let SlotState::Occupied(entry) = state else {
                    unreachable!()
                };
                self.occupied -= 1;
                self.advance_generation(index);
                return Some(entry);
            }
            if matches!(self.slots[index].state, SlotState::Reserved(_)) {
                self.slots[index].state = SlotState::Vacant;
                self.advance_generation(index);
            }
        }
        None
    }

    /// 消费整张表并零分配地迭代已安装项；进程退出路径使用。
    pub fn into_entries(self) -> impl Iterator<Item = Entry<T, R>> {
        self.slots.into_iter().filter_map(|slot| match slot.state {
            SlotState::Occupied(entry) => Some(entry),
            SlotState::Vacant | SlotState::Reserved(_) | SlotState::Retired => None,
        })
    }

    /// 摘出所有已安装项，并清除任何事务预留。测试与非退出路径使用；
    /// 进程最终回收优先消费 [`Self::into_entries`]，避免为清理再分配。
    pub fn drain(&mut self) -> Vec<Entry<T, R>> {
        let mut entries = Vec::new();
        let _ = entries.try_reserve(self.occupied);
        for index in 1..self.slots.len() {
            let state = core::mem::replace(&mut self.slots[index].state, SlotState::Vacant);
            match state {
                SlotState::Occupied(entry) => {
                    entries.push(entry);
                    self.occupied -= 1;
                    self.advance_generation(index);
                }
                SlotState::Reserved(_) => self.advance_generation(index),
                SlotState::Retired => self.slots[index].state = SlotState::Retired,
                SlotState::Vacant => {}
            }
        }
        entries
    }

    fn occupied_index(&self, handle: Handle) -> Result<usize, TableError> {
        if !handle.is_valid() {
            return Err(TableError::InvalidHandle);
        }
        let index = handle.slot() as usize;
        let Some(slot) = self.slots.get(index) else {
            return Err(TableError::StaleHandle);
        };
        if slot.generation != handle.generation() || !matches!(slot.state, SlotState::Occupied(_)) {
            return Err(TableError::StaleHandle);
        }
        Ok(index)
    }

    /// 空槽线性扫描（已知简化）：满表前每次 insert/reserve O(n)，
    /// 65 536 槽上限下可被逐项 close/duplicate 放大；pm 接入形成真实
    /// 负载时收敛为空闲链（notes/impls/ipc.md「Handle 与对象」）。
    fn reserve_slot(&mut self, token: u64) -> Result<Handle, TableError> {
        if let Some(index) =
            (1..self.slots.len()).find(|&i| matches!(self.slots[i].state, SlotState::Vacant))
        {
            let slot = &mut self.slots[index];
            slot.state = SlotState::Reserved(token);
            return Ok(Handle::from_parts(index as u32, slot.generation));
        }
        if self.slots.len().saturating_sub(1) >= self.limit {
            return Err(TableError::ReachLimit);
        }
        self.slots
            .try_reserve(1)
            .map_err(|_| TableError::AllocationFailed)?;
        let index = self.slots.len();
        self.slots.push(Slot {
            generation: 1,
            state: SlotState::Reserved(token),
        });
        Ok(Handle::from_parts(index as u32, 1))
    }

    fn is_reserved(&self, handle: Handle, token: u64) -> bool {
        if !handle.is_valid() {
            return false;
        }
        self.slots.get(handle.slot() as usize).is_some_and(|slot| {
            slot.generation == handle.generation()
                && matches!(slot.state, SlotState::Reserved(actual) if actual == token)
        })
    }

    fn rollback_handles(&mut self, handles: &[Handle], token: u64) {
        for handle in handles.iter().copied() {
            let index = handle.slot() as usize;
            if self.is_reserved(handle, token) {
                self.slots[index].state = SlotState::Vacant;
                self.advance_generation(index);
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

impl<T, R> Default for HandleTable<T, R> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Role {
        Owner,
        Sender,
    }

    fn entry(value: u32, role: Role, rights: Rights) -> Entry<u32, Role> {
        Entry::new(value, role, rights)
    }

    #[test]
    fn stale_handle_never_names_reused_slot() {
        let mut table = HandleTable::with_limit(2);
        let old = table.insert(entry(1, Role::Owner, Rights::READ)).unwrap();
        assert_eq!(table.remove(old).unwrap().object, 1);
        let new = table.insert(entry(2, Role::Owner, Rights::READ)).unwrap();
        assert_eq!(old.slot(), new.slot());
        assert_ne!(old.generation(), new.generation());
        assert_eq!(
            table.get(old, Rights::READ).unwrap_err(),
            TableError::StaleHandle
        );
    }

    #[test]
    fn generation_wrap_retires_slot() {
        let mut table = HandleTable::with_limit(2);
        let first = table.insert(entry(1, Role::Owner, Rights::READ)).unwrap();
        let index = first.slot() as usize;
        table.slots[index].generation = u32::MAX;
        let last = Handle::from_parts(first.slot(), u32::MAX);
        table.remove(last).unwrap();
        assert!(matches!(table.slots[index].state, SlotState::Retired));
        let next = table.insert(entry(2, Role::Owner, Rights::READ)).unwrap();
        assert_ne!(next.slot(), first.slot());
    }

    #[test]
    fn duplicate_requires_right_and_cannot_amplify() {
        let mut table = HandleTable::new();
        let source = table
            .insert(entry(7, Role::Sender, Rights::WRITE | Rights::DUPLICATE))
            .unwrap();
        let copy = table.duplicate(source, Rights::WRITE).unwrap();
        assert_eq!(*table.get(copy, Rights::WRITE).unwrap().object(), 7);
        assert_eq!(
            table
                .duplicate(source, Rights::WRITE | Rights::TRANSFER)
                .unwrap_err(),
            TableError::RightsDenied
        );
    }

    #[test]
    fn failed_move_keeps_every_source() {
        let mut table = HandleTable::new();
        let a = table
            .insert(entry(1, Role::Sender, Rights::WRITE | Rights::TRANSFER))
            .unwrap();
        let b = table.insert(entry(2, Role::Owner, Rights::READ)).unwrap();
        assert_eq!(
            table
                .extract_moves(&[(a, Rights::WRITE), (b, Rights::READ)])
                .unwrap_err(),
            TableError::RightsDenied
        );
        assert!(table.get(a, Rights::WRITE).is_ok());
        assert!(table.get(b, Rights::READ).is_ok());
    }

    #[test]
    fn successful_move_retires_source_values() {
        let mut table = HandleTable::new();
        let a = table
            .insert(entry(1, Role::Sender, Rights::WRITE | Rights::TRANSFER))
            .unwrap();
        let moved = table.extract_moves(&[(a, Rights::WRITE)]).unwrap();
        assert_eq!(moved[0].rights(), Rights::WRITE);
        assert_eq!(
            table.get(a, Rights::NONE).unwrap_err(),
            TableError::StaleHandle
        );
    }

    #[test]
    fn reservation_commit_is_all_or_nothing() {
        let mut table = HandleTable::new();
        let reservation = table.reserve(2, 9).unwrap();
        let handles = reservation.handles().to_vec();
        assert_eq!(table.len(), 0);
        table
            .commit(
                reservation,
                vec![
                    entry(3, Role::Sender, Rights::WRITE),
                    entry(4, Role::Sender, Rights::WRITE),
                ],
            )
            .unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(*table.get(handles[0], Rights::WRITE).unwrap().object(), 3);
        assert_eq!(*table.get(handles[1], Rights::WRITE).unwrap().object(), 4);
    }

    #[test]
    fn rollback_invalidates_temporary_values() {
        let mut table: HandleTable<u32, Role> = HandleTable::new();
        let reservation = table.reserve(1, 11).unwrap();
        let temporary = reservation.handles()[0];
        table.rollback(reservation).unwrap();
        let real = table.insert(entry(5, Role::Owner, Rights::READ)).unwrap();
        assert_eq!(temporary.slot(), real.slot());
        assert_ne!(temporary.generation(), real.generation());
    }

    #[test]
    fn drain_returns_entries_and_clears_reservations() {
        let mut table = HandleTable::new();
        table.insert(entry(1, Role::Owner, Rights::READ)).unwrap();
        let _reservation = table.reserve(1, 17).unwrap();
        let drained = table.drain();
        assert_eq!(drained.len(), 1);
        assert!(table.is_empty());
    }

    #[test]
    fn take_next_needs_no_cleanup_allocation() {
        let mut table = HandleTable::new();
        table.insert(entry(1, Role::Owner, Rights::READ)).unwrap();
        table.insert(entry(2, Role::Owner, Rights::READ)).unwrap();
        let _reservation = table.reserve(1, 23).unwrap();
        let mut cursor = 1;
        let mut values = [0; 2];
        for value in &mut values {
            *value = *table.take_next(&mut cursor).unwrap().object();
        }
        assert_eq!(values, [1, 2]);
        assert!(table.take_next(&mut cursor).is_none());
        assert!(table.is_empty());
    }

    #[test]
    fn limit_is_explicit() {
        let mut table = HandleTable::with_limit(1);
        table.insert(entry(1, Role::Owner, Rights::READ)).unwrap();
        assert_eq!(
            table
                .insert(entry(2, Role::Owner, Rights::READ))
                .unwrap_err(),
            TableError::ReachLimit
        );
    }
}
