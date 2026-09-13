//! 内核对象 Handle 表包装：类型/role/rights 校验与关闭分流。

use alloc::vec::Vec;
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
pub(crate) enum PendingClose {
    Entry(ProcessHandleEntry),
    Retirement(ObjectRef),
}

impl PendingClose {
    pub(crate) fn dependency(&self) -> super::request::FinishDependency {
        match self {
            Self::Retirement(object) => {
                super::request::FinishDependency::Retirement(object.clone())
            }
            Self::Entry(_) => panic!("unstarted entry cannot block on retirement"),
        }
    }

    pub(crate) fn advance(self, owner: &Process, budget: usize) -> (usize, Option<Self>) {
        match self {
            Self::Entry(entry) => retire_entry(entry, owner, budget),
            Self::Retirement(object) => {
                if object
                    .retirement()
                    .expect("retirement ticket lost its backend")
                    .is_finished()
                {
                    (1, None)
                } else {
                    (0, Some(Self::Retirement(object)))
                }
            }
        }
    }
}
pub use handle_table::TakeNext;

pub(crate) struct PendingEntries {
    entries: Option<Vec<ProcessHandleEntry>>,
}

impl PendingEntries {
    pub(crate) fn try_new(capacity: usize) -> Result<Self, SystemCallError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self {
            entries: Some(entries),
        })
    }

    pub(crate) fn from_vec(entries: Vec<ProcessHandleEntry>) -> Self {
        Self {
            entries: Some(entries),
        }
    }

    pub(crate) fn try_reserve(&mut self, additional: usize) -> Result<(), SystemCallError> {
        self.entries
            .as_mut()
            .expect("pending entries already consumed")
            .try_reserve(additional)
            .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn push(&mut self, entry: ProcessHandleEntry) {
        let entries = self
            .entries
            .as_mut()
            .expect("pending entries already consumed");
        assert!(
            entries.len() < entries.capacity(),
            "pending entry capacity was not reserved"
        );
        entries.push(entry);
    }

    pub(crate) fn entries(&self) -> &[ProcessHandleEntry] {
        self.entries
            .as_ref()
            .expect("pending entries already consumed")
    }

    pub(crate) fn close(mut self, owner: &Process, exiting: bool) {
        for entry in self
            .entries
            .take()
            .expect("pending entries already consumed")
        {
            close_entry(entry, owner, exiting);
        }
    }

    pub(crate) fn take(mut self) -> Vec<ProcessHandleEntry> {
        self.entries
            .take()
            .expect("pending entries already consumed")
    }
}

impl IntoIterator for PendingEntries {
    type Item = ProcessHandleEntry;
    type IntoIter = alloc::vec::IntoIter<ProcessHandleEntry>;

    fn into_iter(mut self) -> Self::IntoIter {
        self.entries
            .take()
            .expect("pending entries already consumed")
            .into_iter()
    }
}

impl Drop for PendingEntries {
    fn drop(&mut self) {
        assert!(
            self.entries.is_none(),
            "pending handle entries must be explicitly closed or committed"
        );
    }
}

static NEXT_TRANSACTION: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

pub(crate) fn transaction_token() -> Result<u64, SystemCallError> {
    NEXT_TRANSACTION
        .allocate()
        .ok_or(SystemCallError::ReachLimit)
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

/// 仅描述本进程真实持有的 entry，不通过身份数值打开对象。
pub fn query(thread: &super::Thread, source: Handle, output: usize) -> Result<(), SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(source, Rights::NONE).map_err(map_error)?;
    let description = erhino_shared::object::HandleDescription {
        object_id: entry.object().header().koid(),
        related_object_id: entry.object().related_id(),
        kind: entry.object().kind() as u32,
        role: *entry.role() as u32,
        rights: entry.rights(),
        badge: entry.object().badge(),
        reserved: 0,
    };
    let mut space = thread.process.space.lock();
    // SAFETY: 固定宽字段无 padding，输出失败不改变 capability。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &description) }
        .map_err(Into::into)
}

pub enum HandleCloseStart {
    Ready,
    Wait(super::wait::WaitPlan),
}

/// 表项已从 HandleTable 摘除且表锁已释放；现在执行对象生命周期动作。
pub fn close_entry(entry: ProcessHandleEntry, owner: &Process, exiting: bool) {
    if let Some(target) = entry.object().retirement() {
        assert!(
            exiting,
            "explicit object retirement must use its commit path"
        );
        let launch = target
            .begin(entry.object().clone(), None)
            .expect("detached retirement must remain prepaid");
        launch.publish();
        drop(entry);
        return;
    }
    if entry.object().kind() == super::object::ObjectKind::TunnelEndpoint {
        assert!(
            exiting,
            "explicit Tunnel Endpoint close must use its transaction path"
        );
        super::tunnel::close_detached(entry, owner);
        return;
    }
    let (object, role, _) = entry.into_parts();
    object.close_handle(role, owner, exiting);
}

/// 启动对象执行者并返回完成 ticket；ProcessDrain 不参与内部物理退休。
pub(crate) fn retire_entry(
    entry: ProcessHandleEntry,
    owner: &Process,
    budget: usize,
) -> (usize, Option<PendingClose>) {
    assert!(budget > 0, "entry retirement requires a positive budget");
    if let Some(target) = entry.object().retirement() {
        let object = entry.object().clone();
        let launch = target
            .begin(object.clone(), None)
            .expect("detached retirement must remain prepaid");
        launch.publish();
        drop(entry);
        return (1, Some(PendingClose::Retirement(object)));
    }
    close_entry(entry, owner, true);
    (1, None)
}

pub fn close_entry_infallible(entry: ProcessHandleEntry, owner: &Process, exiting: bool) {
    assert!(
        entry.object().kind() != super::object::ObjectKind::TunnelEndpoint,
        "Tunnel Endpoint must close through its detached path"
    );
    close_entry(entry, owner, exiting);
}

pub fn close_transit(entry: ProcessHandleEntry) {
    let (object, role, _) = entry.into_parts();
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
    let entry = {
        let mut table = thread.process.handles.lock();
        let entry = table.get(handle, Rights::NONE).map_err(map_error)?;
        if let Some(target) = entry.object().retirement() {
            let launch = target.begin(entry.object().clone(), Some(thread.process.clone()))?;
            let entry = table
                .remove(handle)
                .expect("retirement commit must retain its handle slot");
            drop(table);
            drop(entry);
            let plan = launch
                .publish()
                .expect("explicit retirement lost its reply");
            return Ok(HandleCloseStart::Wait(plan));
        }
        table.remove(handle).map_err(map_error)?
    };
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
    let token = transaction_token()?;
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

/// 向调用者原子发布一个 Handle：先预留不可见槽并写回槽值，再执行不可失败
/// 的对象提交，最后公开表项。publish 在 HandleTable 锁内执行，不得反向取低秩锁
/// 或分配；失败返回时 publish 尚未发生。
pub(crate) fn install_one(
    thread: &super::Thread,
    entry: ProcessHandleEntry,
    output: usize,
    publish: impl FnOnce(),
) -> Result<(), SystemCallError> {
    let mut entries = Vec::new();
    entries
        .try_reserve(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(entry);
    let token = transaction_token()?;
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(1, token).map_err(map_error)?;
    let handle = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table
            .rollback(reservation)
            .expect("single-handle install reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding；复检失败即杀本进程，未提交的预留随进程消亡。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &handle) }?;
    drop(space);
    // 输出中的槽此刻仍为 Reserved；先完成对象提交，再公开 capability。
    publish();
    table
        .commit(reservation, entries)
        .expect("single-handle install count matches entry");
    Ok(())
}

pub fn map_error(error: TableError) -> SystemCallError {
    match error {
        TableError::InvalidHandle => SystemCallError::IllegalArgument,
        TableError::StaleHandle => SystemCallError::StaleHandle,
        TableError::ObjectBusy => SystemCallError::ObjectBusy,
        TableError::RightsDenied => SystemCallError::RightsDenied,
        TableError::DuplicateHandle => SystemCallError::IllegalArgument,
        TableError::ReachLimit => SystemCallError::ReachLimit,
        TableError::BadReservation => SystemCallError::InternalError,
        TableError::AllocationFailed => SystemCallError::OutOfMemory,
    }
}
