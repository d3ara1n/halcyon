//! 进程本地 capability 表的内核拼装：`CapTable<ObjectAuthority>` 与系统调用入口。
//!
//! 设计见 plans/todo-2026-09-28-capability-last-reference.md：
//! - 角色即对象类型（设计轮次 4）：表项只有对象与 rights，无 role；
//! - 铸造经 [`mint`] 校验对象 rights 上限；复制经 `Cap::derive`（同对象、rights 子集）；
//! - 所有放弃（Close、事务回滚、在途丢弃、进程回收）都经 `Cap::release`，
//!   最后持有事件在表锁外经 [`cap::deliver`] 交对象执行。

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

/// 一次已接受的非消费授权；存储引用与 BodyLease 都不计入 capability 持有数。
/// 需跨 Close 的业务责任必须由 accept_with 取得独立领域凭证，不能由 ObjectRef 代替。
pub(crate) struct Authorization {
    body: super::process::shell::BodyLease,
    object: ObjectRef,
    rights: Rights,
}

impl Authorization {
    pub(crate) fn accept(
        thread: &super::Thread,
        handle: Handle,
        rights: Rights,
    ) -> Result<Self, SystemCallError> {
        Self::accept_with(thread, handle, rights, |_, _| Ok(())).map(|(authority, ())| authority)
    }

    /// 表内校验与短领域凭证取得不可被最后 Close 插入。
    /// prepare 只允许 Lifetime 接受/派生或 backing use 准入；禁止 uaccess、
    /// 表重入、维护发布、capability 释放及任意业务提交。返回值必须独立拥有责任。
    pub(crate) fn accept_with<T>(
        thread: &super::Thread,
        handle: Handle,
        required: Rights,
        prepare: impl FnOnce(&ObjectRef, Rights) -> Result<T, SystemCallError>,
    ) -> Result<(Self, T), SystemCallError> {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let (object, rights, prepared) = {
            let table = body.handles.lock();
            let cap = table.get(handle, required).map_err(map_error)?;
            let object = cap.object().0.clone();
            let rights = cap.rights();
            let prepared = prepare(&object, rights)?;
            (object, rights, prepared)
        };
        Ok((
            Self {
                body,
                object,
                rights,
            },
            prepared,
        ))
    }

    pub(crate) fn body(&self) -> &super::process::shell::BodyLease {
        &self.body
    }

    pub(crate) fn object(&self) -> &ObjectRef {
        &self.object
    }

    pub(crate) fn rights(&self) -> Rights {
        self.rights
    }
}

/// 表锁外执行的操作 authority；BodyLease 覆盖接受/返还全过程。
/// Close/shut 只能撤销返还资格，不能取消已接管的业务调用。
pub(crate) struct Invocation {
    body: super::process::shell::BodyLease,
    cap: Option<Cap>,
    ticket: Option<capability::ClaimTicket>,
}

impl Invocation {
    pub(crate) fn claim(
        thread: &super::Thread,
        handle: Handle,
        rights: Rights,
    ) -> Result<Self, SystemCallError> {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let (cap, ticket) = body
            .handles
            .lock()
            .claim(handle, rights)
            .map_err(map_error)?;
        Ok(Self {
            body,
            cap: Some(cap),
            ticket: Some(ticket),
        })
    }
    pub(crate) fn body(&self) -> &super::process::shell::BodyLease {
        &self.body
    }
    pub(crate) fn cap(&self) -> &Cap {
        self.cap.as_ref().expect("invocation lost authority")
    }
    pub(crate) fn object(&self) -> &ObjectRef {
        &self.cap().object().0
    }
    pub(crate) fn consume(&mut self) {
        self.body
            .handles
            .lock()
            .commit_claim(self.ticket.take().expect("invocation lost ticket"));
        cap::deliver(
            self.cap
                .take()
                .expect("invocation lost authority")
                .release(),
        );
    }
    pub(crate) fn take_for_transfer(&mut self) -> Cap {
        self.cap.take().expect("invocation transferred twice")
    }
    pub(crate) fn restore_transfer(&mut self, cap: Cap) {
        assert!(
            self.cap.is_none() && self.ticket.is_some(),
            "invalid transfer rollback"
        );
        self.cap = Some(cap);
    }
    pub(crate) fn commit_transfer(&mut self) {
        assert!(
            self.cap.is_none(),
            "transfer committed before owner movement"
        );
        self.body
            .handles
            .lock()
            .commit_claim(self.ticket.take().expect("transfer lost ticket"));
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        if let Some(cap) = self.cap.take() {
            let restored = self
                .body
                .handles
                .lock()
                .restore(self.ticket.take().expect("invocation lost ticket"), cap);
            if let capability::Restore::Released(last) = restored {
                cap::deliver(last);
            }
        } else {
            assert!(
                self.ticket.is_none(),
                "transferred authority lost commit or rollback"
            );
        }
    }
}

/// 铸造一份 authority：rights 必须在对象类型上限之内。
pub(crate) fn mint(object: ObjectRef, rights: Rights) -> Result<Cap, SystemCallError> {
    let allowed = object
        .allowed_rights()
        .ok_or(SystemCallError::RightsDenied)?;
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
/// 表锁只覆盖 reserve/rollback/commit；写回与必成的对象出生提交在表锁外。
/// 预约后返还资格由 shut 撤销时，提交返回释放事件，不能推翻对象责任。
pub(crate) fn install<I>(
    thread: &super::Thread,
    caps: I,
    write: impl FnOnce(&[Handle]) -> Result<(), SystemCallError>,
    publish: impl FnOnce(),
) -> Result<funded_storage::Buffer<Handle, super::resources::MetadataCharge>, SystemCallError>
where
    I: IntoIterator<Item = Cap>,
    I::IntoIter: ExactSizeIterator,
{
    let caps = caps.into_iter();
    let body = match thread.process.lease_body() {
        Some(body) => body,
        None => {
            release_all(caps);
            return Err(SystemCallError::ObjectClosed);
        }
    };
    let mut table = body.handles.lock();
    let reservation = match table.reserve(
        caps.len(),
        &super::resources::MetadataFunding(thread.process.pool()),
    ) {
        Ok(reservation) => reservation,
        Err(error) => {
            drop(table);
            release_all(caps);
            return Err(map_error(error));
        }
    };
    drop(table);
    if let Err(error) = write(reservation.handles()) {
        body.handles.lock().rollback(reservation);
        release_all(caps);
        return Err(error);
    }
    publish();
    let committed = body.handles.lock().commit(reservation, caps);
    match committed {
        Ok(capability::Commit::Installed(handles)) => Ok(handles),
        Ok(capability::Commit::Released { handles, events }) => {
            for event in events {
                super::cap::deliver(Some(event));
            }
            Ok(handles)
        }
        Err((caps, error)) => {
            release_all(caps);
            Err(map_error(error))
        }
    }
}

/// 只描述本进程真实持有的 capability，不通过身份数值打开对象。
pub fn query(thread: &super::Thread, source: Handle, output: usize) -> Result<(), SystemCallError> {
    let (object, rights) = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let table = body.handles.lock();
        let cap = table.get(source, Rights::NONE).map_err(map_error)?;
        (cap.object().0.clone(), cap.rights())
    };
    // 描述只借存储、不产生 authority；related_id/badge 的领域同步在表锁外。
    let description = erhino_shared::object::HandleDescription {
        object_id: object.header().koid(),
        related_object_id: object.related_id(),
        kind: object.kind() as u32,
        reserved0: 0,
        rights,
        badge: object.badge(),
        reserved: 0,
    };
    crate::uaccess::write_user(thread, output, &description)
}

/// Close：合法 owner 必成、不等待。已被在途操作接管的槽只撤销返还资格。
pub fn close(thread: &super::Thread, handle: Handle) -> Result<(), SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let taken = body.handles.lock().close(handle).map_err(map_error)?;
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
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let table = body.handles.lock();
        table
            .get(source, Rights::NONE)
            .map_err(map_error)?
            .derive(rights)
            .map_err(map_error)?
    };
    let caps = [derived];
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
        TableError::FundingDenied => SystemCallError::QuotaExceeded,
        TableError::Closed | TableError::Sealed => SystemCallError::ObjectClosed,
    }
}
