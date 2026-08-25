//! 内核对象 Handle 表包装：类型/role/rights 校验与关闭分流。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use handle_table::{Entry, HandleTable, TableError};

use super::{
    object::{HandleRole, ObjectRef},
    proc::Process,
};

pub type ProcessHandleTable = HandleTable<ObjectRef, HandleRole>;
pub type ProcessHandleEntry = Entry<ObjectRef, HandleRole>;

static NEXT_TRANSACTION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn transaction_token() -> u64 {
    let token = NEXT_TRANSACTION.fetch_add(1, Ordering::Relaxed);
    assert!(token != 0, "Handle transaction identity exhausted");
    token
}

/// 构造一项已经过对象 role 与最大 rights 校验的表项。
pub fn entry(
    object: ObjectRef,
    role: HandleRole,
    rights: Rights,
) -> Result<ProcessHandleEntry, TableError> {
    if !rights.is_known() {
        return Err(TableError::RightsDenied);
    }
    let allowed = object
        .allowed_rights(role)
        .ok_or(TableError::RightsDenied)?;
    if !rights.is_subset_of(allowed) {
        return Err(TableError::RightsDenied);
    }
    Ok(Entry::new(object, role, rights))
}

/// 表项已从 HandleTable 摘除且表锁已释放；现在执行对象生命周期动作。
pub fn close_entry(entry: ProcessHandleEntry, owner: &Process, exiting: bool) {
    let (object, role, _) = entry.into_parts();
    object.close_handle(role, owner, exiting);
}

pub fn close_transit(entry: ProcessHandleEntry) {
    let (object, role, _) = entry.into_parts();
    object.close_transit(role);
}

pub fn close(thread: &super::Thread, handle: Handle) -> Result<(), SystemCallError> {
    let entry = thread.process.handles.lock().remove(handle).map_err(map_error)?;
    close_entry(entry, &thread.process, false);
    Ok(())
}

pub fn duplicate(
    thread: &super::Thread,
    source: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let mut entries = Vec::new();
    entries.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
    let token = transaction_token();
    let mut table = thread.process.handles.lock();
    entries.push(table.derive(source, rights).map_err(map_error)?);
    let reservation = table.reserve(1, token).map_err(map_error)?;
    let duplicated = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table.rollback(reservation).expect("duplicate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 是无 padding 的 u64 newtype，输出已在同一 space 锁下校验。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &duplicated) }
        .expect("validated duplicate output must remain writable");
    drop(space);
    table.commit(reservation, entries).expect("duplicate reservation must remain owned");
    Ok(())
}

pub fn map_error(error: TableError) -> SystemCallError {
    match error {
        TableError::InvalidHandle => SystemCallError::IllegalArgument,
        TableError::StaleHandle => SystemCallError::StaleHandle,
        TableError::RightsDenied => SystemCallError::RightsDenied,
        TableError::DuplicateHandle => SystemCallError::IllegalArgument,
        TableError::ReachLimit => SystemCallError::ReachLimit,
        TableError::BadReservation => SystemCallError::InternalError,
        TableError::AllocationFailed => SystemCallError::OutOfMemory,
    }
}
