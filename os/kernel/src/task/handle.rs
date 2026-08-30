//! 内核对象 Handle 表包装：类型/role/rights 校验与关闭分流。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};

use handle_table::{Entry, HandleTable, TableError};

use super::{
    object::{HandleRole, ObjectRef},
    proc::Process,
};

pub type ProcessHandleTable = HandleTable<ObjectRef, HandleRole>;
pub type ProcessHandleEntry = Entry<ObjectRef, HandleRole>;
pub use handle_table::TakeNext;

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

/// 构造带对象专用不可变 badge 的表项；badge 随 duplicate/move 保持。
pub fn entry_with_badge(
    object: ObjectRef,
    role: HandleRole,
    rights: Rights,
    badge: u64,
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
    Ok(Entry::new_with_badge(object, role, rights, badge))
}

pub enum HandleCloseStart {
    Ready,
    Wait(super::wait::WaitPlan),
}

/// 表项已从 HandleTable 摘除且表锁已释放；现在执行对象生命周期动作。
/// Tunnel Endpoint 的 lease close 可在 Commit 前失败，此时原样返还 detached entry。
pub fn close_entry(
    entry: ProcessHandleEntry,
    owner: &Process,
    exiting: bool,
) -> Result<(), ProcessHandleEntry> {
    if entry.object().kind() == super::object::ObjectKind::TunnelEndpoint {
        assert!(
            exiting,
            "explicit Tunnel Endpoint close must use its transaction path"
        );
        return super::tunnel::close_detached(entry, owner);
    }
    let (object, role, _, _) = entry.into_parts();
    object.close_handle(role, owner, exiting);
    Ok(())
}

pub fn close_entry_infallible(entry: ProcessHandleEntry, owner: &Process, exiting: bool) {
    assert!(
        entry.object().kind() != super::object::ObjectKind::TunnelEndpoint,
        "Tunnel Endpoint must close through its lease transaction"
    );
    assert!(
        close_entry(entry, owner, exiting).is_ok(),
        "non-Tunnel close cannot require retry"
    );
}

pub fn close_transit(entry: ProcessHandleEntry) {
    let (object, role, _, _) = entry.into_parts();
    object.close_transit(role);
}

pub fn close(thread: &super::Thread, handle: Handle) -> Result<HandleCloseStart, SystemCallError> {
    let tunnel = {
        let table = thread.process.handles.lock();
        let entry = table.get(handle, Rights::NONE).map_err(map_error)?;
        entry.object().kind() == super::object::ObjectKind::TunnelEndpoint
    };
    if tunnel {
        return super::tunnel::close_handle(thread, handle).map(HandleCloseStart::Wait);
    }
    let entry = thread
        .process
        .handles
        .lock()
        .remove(handle)
        .map_err(map_error)?;
    close_entry_infallible(entry, &thread.process, false);
    Ok(HandleCloseStart::Ready)
}

pub fn duplicate(
    thread: &super::Thread,
    source: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let mut entries = Vec::new();
    entries
        .try_reserve(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    let token = transaction_token();
    let mut table = thread.process.handles.lock();
    entries.push(table.derive(source, rights).map_err(map_error)?);
    let reservation = table.reserve(1, token).map_err(map_error)?;
    let duplicated = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table
            .rollback(reservation)
            .expect("duplicate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 是无 padding 的 u64 newtype；复检失败即杀本进程
    // （deliver_output），未提交的预留随进程消亡。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &duplicated) }?;
    drop(space);
    table
        .commit(reservation, entries)
        .expect("duplicate reservation must remain owned");
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
