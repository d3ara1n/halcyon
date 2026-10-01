//! 进程本地 capability 表的内核拼装：`CapTable<ObjectAuthority>` 与系统调用入口。
//!
//! 设计见 plans/todo-2026-09-28-capability-last-reference.md：
//! - 角色即对象类型（设计轮次 4）：表项只有对象与 rights，无 role；
//! - 铸造经 [`mint`] 校验对象 rights 上限；复制经 `Cap::derive`（同对象、rights 子集）；
//! - 所有放弃（Close、事务回滚、在途丢弃、进程回收）都经 `Cap::release`，
//!   最后持有事件在表锁外经 [`cap::deliver`] 交对象执行。

use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};

use capability::TableError;

use super::{
    cap::{self, Cap, CapTable, ObjectAuthority},
    object::ObjectRef,
};

pub(crate) type ProcessCapTable = CapTable;

/// 铸造一份 authority：rights 必须在对象类型上限之内。
pub(crate) fn mint(object: ObjectRef, rights: Rights) -> Result<Cap, SystemCallError> {
    let allowed = object.allowed_rights().ok_or(SystemCallError::RightsDenied)?;
    if !rights.is_known() || !rights.is_subset_of(allowed) {
        return Err(SystemCallError::RightsDenied);
    }
    Cap::mint(ObjectAuthority(object), rights).map_err(map_error)
}

/// 放弃一批 authority（锁外调用）。
pub(crate) fn release_all(caps: impl IntoIterator<Item = Cap>) {
    for cap in caps {
        cap::deliver(cap.release());
    }
}

/// 按对象类型解析：校验 rights 与 kind，返回对象存储引用（不产生 authority）。
pub(crate) fn resolve(
    table: &ProcessCapTable,
    handle: Handle,
    rights: Rights,
    kind: super::object::ObjectKind,
) -> Result<ObjectRef, SystemCallError> {
    let cap = table.get(handle, rights).map_err(map_error)?;
    let object = &cap.object().0;
    if object.kind() != kind {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(object.clone())
}

/// 原子发布一批 capability：预留不可见槽 → 写回槽值 → 对象提交 → 公开。
///
/// `write` 在表锁内执行用户写回（失败则回滚预留并返还 caps 给调用方释放）；
/// `publish` 是不可失败的对象侧提交（如 armed 位），在公开前执行。
pub(crate) fn install(
    thread: &super::Thread,
    caps: Vec<Cap>,
    write: impl FnOnce(&[Handle]) -> Result<(), SystemCallError>,
    publish: impl FnOnce(),
) -> Result<Vec<Handle>, SystemCallError> {
    let mut table = thread.process.handles.lock();
    let reservation = match table.reserve(caps.len()) {
        Ok(reservation) => reservation,
        Err(error) => {
            drop(table);
            release_all(caps);
            return Err(map_error(error));
        }
    };
    if let Err(error) = write(reservation.handles()) {
        table.rollback(reservation);
        drop(table);
        release_all(caps);
        return Err(error);
    }
    publish();
    match table.commit(reservation, caps) {
        Ok(handles) => Ok(handles),
        Err((caps, error)) => {
            drop(table);
            release_all(caps);
            Err(map_error(error))
        }
    }
}

/// 只描述本进程真实持有的 capability，不通过身份数值打开对象。
pub fn query(thread: &super::Thread, source: Handle, output: usize) -> Result<(), SystemCallError> {
    let description = {
        let table = thread.process.handles.lock();
        let cap = table.get(source, Rights::NONE).map_err(map_error)?;
        let object = &cap.object().0;
        erhino_shared::object::HandleDescription {
            object_id: object.header().koid(),
            related_object_id: object.related_id(),
            kind: object.kind() as u32,
            reserved0: 0,
            rights: cap.rights(),
            badge: object.badge(),
            reserved: 0,
        }
    };
    crate::uaccess::write_user(thread, output, &description)
}

/// Close：合法 owner 必成、不等待。已被在途操作接管的槽只撤销返还资格。
pub fn close(thread: &super::Thread, handle: Handle) -> Result<(), SystemCallError> {
    let taken = thread.process.handles.lock().close(handle).map_err(map_error)?;
    if let Some(cap) = taken {
        cap::deliver(cap.release());
    }
    Ok(())
}

pub fn duplicate(
    thread: &super::Thread,
    source: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let derived = {
        let table = thread.process.handles.lock();
        table
            .get(source, Rights::NONE)
            .map_err(map_error)?
            .derive(rights)
            .map_err(map_error)?
    };
    let mut caps = Vec::new();
    if caps.try_reserve_exact(1).is_err() {
        cap::deliver(derived.release());
        return Err(SystemCallError::OutOfMemory);
    }
    caps.push(derived);
    install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}

pub fn map_error(error: TableError) -> SystemCallError {
    match error {
        TableError::InvalidHandle | TableError::DuplicateHandle => SystemCallError::IllegalArgument,
        TableError::StaleHandle => SystemCallError::StaleHandle,
        TableError::Claimed => SystemCallError::ObjectBusy,
        TableError::RightsDenied => SystemCallError::RightsDenied,
        TableError::ReachLimit => SystemCallError::ReachLimit,
        TableError::BadReservation => SystemCallError::InternalError,
        TableError::AllocationFailed => SystemCallError::OutOfMemory,
        TableError::Closed | TableError::Sealed => SystemCallError::ObjectClosed,
    }
}
