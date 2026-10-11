//! 服务名字/注册的唯一 backing；发现直接借用当前注册，不分配文件节点。
use crate::{
    authority::{DirectoryAccess, NameAuthority},
    protocol::{
        self, AccessRights, Descriptors, PublishRights, Reason, RegisterMode, RegistrationId,
        RegistrationInfo, State, Status,
    },
    resource::ServiceResource,
};
use alloc::{rc::Rc, string::String, sync::Arc, vec::Vec};
use core::{borrow::Borrow, cmp::Ordering, mem::ManuallyDrop};
use erhino_shared::{call::SystemCallError, object::Rights};
use libbudget::Charge;
use libkernel::ipc::message::MailboxSender;
use liboal::{MarkerId, ObjectInfo, Tag};
use librpc::bytes::{Reader, Writer};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

pub const NAMESPACE: u128 = 0x48414c43594f4e000000000053525600;
pub const ENTRY: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 1,
};
pub const REGISTRATION: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 1,
};
pub const PROTOCOLS: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 2,
};
fn system(e: SystemCallError) -> Status {
    match e {
        SystemCallError::QuotaExceeded => Status::Quota,
        SystemCallError::RightsDenied => Status::Permission,
        SystemCallError::ObjectClosed => Status::Unavailable,
        _ => Status::Resource,
    }
}
fn copy(s: &str) -> Result<String, Status> {
    let mut value = String::new();
    value
        .try_reserve_exact(s.len())
        .map_err(|_| Status::Resource)?;
    value.push_str(s);
    Ok(value)
}
fn bytes(s: &[u8]) -> Result<Vec<u8>, Status> {
    let mut value = Vec::new();
    value
        .try_reserve_exact(s.len())
        .map_err(|_| Status::Resource)?;
    value.extend_from_slice(s);
    Ok(value)
}
fn entry_error<V>(e: ordered_table::InsertError<V, core::convert::Infallible>) -> Status {
    match e {
        ordered_table::InsertError::Limit(_) => Status::Quota,
        ordered_table::InsertError::Storage(e, _) => system(libbudget::allocator_error(e)),
    }
}

struct Name {
    value: String,
    _charge: Charge,
}
#[derive(Clone)]
struct NameKey(Rc<Name>);
impl Borrow<str> for NameKey {
    fn borrow(&self) -> &str {
        &self.0.value
    }
}
impl PartialEq for NameKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.value == other.0.value
    }
}
impl Eq for NameKey {}
impl PartialOrd for NameKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NameKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.value.cmp(&other.0.value)
    }
}
struct ActiveBody {
    name: NameKey,
    descriptors: Vec<u8>,
    endpoint: Option<MailboxSender>,
    export_transport: Rights,
    _bytes: Charge,
}
struct Registration {
    info: RegistrationInfo,
    task: u64,
    body: Option<ActiveBody>,
    endpoint_removed: bool,
    control_closed: bool,
    control_removed: bool,
    _slot: Permit,
    _unit: Charge,
    _bytes: Charge,
    _sources: Charge,
    funding: crate::Funding,
}
struct Named {
    context: u64,
    _slot: Option<Permit>,
    _bytes: Charge,
}
pub struct PreparedRegistration {
    authority: NameAuthority,
    mode: RegisterMode,
    record: PreparedEntry<Registration, ()>,
    name: PreparedEntry<Named, (), NameKey>,
}
impl PreparedRegistration {
    pub fn set_task(&mut self, task: u64) {
        self.record.value_mut().task = task;
    }
    pub fn info(&self) -> RegistrationInfo {
        self.record.value().info
    }
    pub fn context(&self) -> u64 {
        self.record.key()
    }
    pub fn endpoint_handle(&self) -> erhino_shared::object::Handle {
        self.record
            .value()
            .body
            .as_ref()
            .and_then(|body| body.endpoint.as_ref())
            .expect("prepared registration endpoint missing")
            .as_handle()
    }
}
pub struct RegisterFailure {
    pub status: Status,
    pub prepared: PreparedRegistration,
}
pub struct EntrySnapshot {
    pub id: RegistrationId,
    pub descriptors: Vec<u8>,
    pub endpoint: MailboxSender,
    pub transport: Rights,
}

pub struct Registry {
    incarnation: u64,
    serial: monotonic_id::AtomicId64,
    registrations: ManuallyDrop<OrderedTable<Registration, ()>>,
    names: ManuallyDrop<OrderedTable<Named, (), NameKey>>,
    slots: Arc<Counter>,
    name_slots: Arc<Counter>,
    sealed: bool,
    pub watches: crate::watch::WatchTable,
}
impl Registry {
    pub fn new(incarnation: u64, limit: usize, watch_limit: usize) -> Result<Self, Status> {
        if incarnation == 0 || limit == 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            incarnation,
            serial: monotonic_id::AtomicId64::new(1),
            registrations: ManuallyDrop::new(OrderedTable::new(limit)),
            names: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            name_slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            sealed: false,
            watches: crate::watch::WatchTable::new(watch_limit)?,
        })
    }
    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }
    pub fn admit(name: &str, descriptors: Descriptors<'_>) -> Result<(), Status> {
        if !protocol::valid_name(name) {
            return Err(Status::Invalid);
        }
        let info = Self::info_len(descriptors)?;
        let page = 36usize
            .checked_add(name.len().checked_mul(2).ok_or(Status::EntryTooLarge)?)
            .and_then(|n| n.checked_add(info))
            .ok_or(Status::EntryTooLarge)?;
        if page > protocol::BODY_MAX
            || 24usize
                .checked_add(descriptors.as_bytes().len())
                .is_none_or(|n| n > protocol::BODY_MAX)
        {
            return Err(Status::EntryTooLarge);
        }
        Ok(())
    }
    fn info_len(descriptors: Descriptors<'_>) -> Result<usize, Status> {
        92usize
            .checked_add(descriptors.as_bytes().len())
            .ok_or(Status::SummaryTooLarge)
    }
    pub fn prepare_register(
        &mut self,
        authority: &NameAuthority,
        context: u64,
        request: &protocol::Request<'_>,
        endpoint: MailboxSender,
    ) -> Result<PreparedRegistration, Status> {
        let protocol::Request::Register {
            name,
            mode,
            descriptors,
            export_transport,
        } = *request
        else {
            return Err(Status::Invalid);
        };
        if self.sealed {
            return Err(Status::Revoked);
        }
        Self::admit(name, descriptors)?;
        let required = if mode == RegisterMode::Replace {
            PublishRights::CREATE | PublishRights::REPLACE
        } else {
            PublishRights::CREATE
        };
        authority.check(Some(name), required)?;
        let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let description = endpoint.description().map_err(system)?;
        if context == 0 || !export_transport.is_known() {
            return Err(Status::Invalid);
        }
        if !description.rights.contains(mandatory | Rights::DUPLICATE)
            || !export_transport.contains(mandatory)
            || !export_transport.is_subset_of(description.rights)
            || !export_transport.is_subset_of(authority.output())
        {
            return Err(Status::Permission);
        }
        if self.registrations.get(context).is_some() {
            return Err(Status::Conflict);
        }
        if mode == RegisterMode::CreateOnly && self.names.get_by(name).is_some() {
            return Err(Status::Exists);
        }
        let account = authority.account();
        let slot = Counter::try_acquire(&self.slots).map_err(|_| Status::Quota)?;
        let unit = account
            .acquire(ServiceResource::Registration, 1)
            .map_err(system)?;
        let sources = account
            .acquire(ServiceResource::WaitSource, 2)
            .map_err(system)?;
        let record_bytes = account
            .acquire(
                ServiceResource::Bytes,
                PreparedEntry::<Registration, ()>::allocation_bytes(),
            )
            .map_err(system)?;
        let owned_name = copy(name)?;
        let name_charge = account
            .acquire(
                ServiceResource::Bytes,
                core::mem::size_of::<Name>()
                    .checked_add(owned_name.capacity())
                    .ok_or(Status::Resource)?,
            )
            .map_err(system)?;
        let key = NameKey(
            Rc::try_new(Name {
                value: owned_name,
                _charge: name_charge,
            })
            .map_err(|_| Status::Resource)?,
        );
        let descriptor_bytes = bytes(descriptors.as_bytes())?;
        let body_charge = account
            .acquire(ServiceResource::Bytes, descriptor_bytes.capacity())
            .map_err(system)?;
        let serial = self.serial.allocate().ok_or(Status::Resource)?;
        let info = RegistrationInfo {
            id: RegistrationId {
                registry_incarnation: self.incarnation,
                serial,
            },
            state: State::Active,
            reason: Reason::None,
        };
        let record = self
            .registrations
            .prepare_insert(
                context,
                Registration {
                    info,
                    task: 0,
                    body: Some(ActiveBody {
                        name: key.clone(),
                        descriptors: descriptor_bytes,
                        endpoint: Some(endpoint),
                        export_transport,
                        _bytes: body_charge,
                    }),
                    endpoint_removed: false,
                    control_closed: false,
                    control_removed: false,
                    _slot: slot,
                    _unit: unit,
                    _bytes: record_bytes,
                    _sources: sources,
                    funding: authority.funding().clone(),
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(entry_error)?;
        let name_slot = if self.names.get_by(name).is_some() {
            None
        } else {
            Some(Counter::try_acquire(&self.name_slots).map_err(|_| Status::Quota)?)
        };
        let charge = account
            .acquire(
                ServiceResource::Bytes,
                PreparedEntry::<Named, (), NameKey>::allocation_bytes(),
            )
            .map_err(system)?;
        let name = self
            .names
            .prepare_insert_candidate(
                key,
                Named {
                    context,
                    _slot: name_slot,
                    _bytes: charge,
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(entry_error)?;
        Ok(PreparedRegistration {
            authority: authority.clone(),
            mode,
            record,
            name,
        })
    }
    /// 两个真实来源与 Outbox 先准入；调用者最终复核期限及已观察的端点关闭。
    pub fn commit(
        &mut self,
        mut prepared: PreparedRegistration,
    ) -> Result<RegistrationInfo, RegisterFailure> {
        let name: &str = prepared.name.key_ref().borrow();
        let required = if prepared.mode == RegisterMode::Replace {
            PublishRights::CREATE | PublishRights::REPLACE
        } else {
            PublishRights::CREATE
        };
        let check = prepared
            .authority
            .check(Some(name), required)
            .and_then(|()| {
                if self.sealed {
                    Err(Status::Revoked)
                } else if prepared.mode == RegisterMode::CreateOnly
                    && self.names.get_by(name).is_some()
                {
                    Err(Status::Exists)
                } else if prepared.name.value()._slot.is_none() && self.names.get_by(name).is_none()
                {
                    Err(Status::Conflict)
                } else {
                    Ok(())
                }
            });
        if let Err(status) = check {
            return Err(RegisterFailure { status, prepared });
        }
        let key = prepared.name.key_ref().clone();
        let event = if let Some(old) = self.names.remove_by(name) {
            let old = old.into_value();
            prepared.name.value_mut()._slot = old._slot;
            self.withdraw_record(old.context, Reason::Replaced);
            protocol::REPLACED
        } else {
            protocol::REGISTERED
        };
        let info = prepared.record.value().info;
        self.registrations.insert_prepared(prepared.record);
        self.names.insert_prepared(prepared.name);
        self.watches.publish(key.borrow(), event);
        Ok(info)
    }
    fn withdraw_record(&mut self, context: u64, reason: Reason) {
        if let Some(record) = self.registrations.get_mut(context)
            && record.info.state == State::Active
        {
            record.info.state = State::Withdrawn;
            record.info.reason = reason;
        }
    }
    pub fn withdraw(&mut self, context: u64, reason: Reason) -> Result<RegistrationInfo, Status> {
        let record = self.registrations.get(context).ok_or(Status::Permission)?;
        let key = record.body.as_ref().map(|b| b.name.clone());
        if let Some(key) = key
            && self
                .names
                .get_by::<str>(key.borrow())
                .is_some_and(|n| n.context == context)
        {
            self.names.remove_by::<str>(key.borrow());
            self.watches.publish(key.borrow(), protocol::WITHDRAWN);
        }
        self.withdraw_record(context, reason);
        self.query(context)
    }
    pub fn query(&self, context: u64) -> Result<RegistrationInfo, Status> {
        self.registrations
            .get(context)
            .map(|r| r.info)
            .ok_or(Status::Permission)
    }
    /// 注册控制和 Withdrawn 壳沿用注册时冻结的付款绑定，不依赖原发布授权存活。
    pub fn control_funding(&self, context: u64) -> Option<&crate::Funding> {
        self.registrations
            .get(context)
            .map(|record| &record.funding)
    }
    pub fn endpoint_handle(&self, context: u64) -> Option<erhino_shared::object::Handle> {
        self.registrations
            .get(context)?
            .body
            .as_ref()?
            .endpoint
            .as_ref()
            .map(MailboxSender::as_handle)
    }
    pub fn get_entry(&self, access: &DirectoryAccess, path: &str) -> Result<EntrySnapshot, Status> {
        access.check(None, AccessRights::GET_ENTRY)?;
        let name = self.leaf_path(access, path, AccessRights::GET_ENTRY)?;
        let named = self.names.get_by(name).ok_or(Status::NotFound)?;
        let record = self
            .registrations
            .get(named.context)
            .ok_or(Status::Internal)?;
        let body = record.body.as_ref().ok_or(Status::Internal)?;
        let transport = body.export_transport & access.output();
        if !transport.contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT) {
            return Err(Status::Permission);
        }
        let descriptors = bytes(&body.descriptors)?;
        let endpoint = body.endpoint.as_ref().ok_or(Status::Unavailable)?;
        let handle =
            libkernel::ipc::object::duplicate(endpoint.as_handle(), transport).map_err(system)?;
        // SAFETY: duplicate 返回本调用唯一拥有的新、未映射 handle，下面完整接管关闭责任。
        let cap = unsafe { libkernel::ipc::capability::Capability::from_raw(handle) };
        let (endpoint, _) = MailboxSender::from_capability(cap).map_err(|e| system(e.error))?;
        Ok(EntrySnapshot {
            id: record.info.id,
            descriptors,
            endpoint,
            transport,
        })
    }
    fn leaf_path<'a>(
        &self,
        access: &DirectoryAccess,
        path: &'a str,
        required: AccessRights,
    ) -> Result<&'a str, Status> {
        liboal::path::validate_path(path).map_err(|_| Status::Invalid)?;
        if path.is_empty() {
            access.check(None, required)?;
            return Err(Status::TypeMismatch);
        }
        let name = path.split('/').next().ok_or(Status::Invalid)?;
        access.check(Some(name), required)?;
        if path.contains('/') {
            return Err(Status::NotDirectory);
        }
        Ok(path)
    }
    fn info(&self, context: u64, out: &mut [u8]) -> Result<usize, Status> {
        let record = self.registrations.get(context).ok_or(Status::NotFound)?;
        let body = record.body.as_ref().ok_or(Status::NotFound)?;
        let mut id = [0; 16];
        record.info.id.write(&mut Writer::new(&mut id));
        liboal::info::encode(
            &[ENTRY],
            &[
                Tag {
                    key: REGISTRATION,
                    value: &id,
                },
                Tag {
                    key: PROTOCOLS,
                    value: &body.descriptors,
                },
            ],
            out,
        )
        .map_err(|_| Status::SummaryTooLarge)
    }
    pub fn lookup<'a>(
        &self,
        access: &DirectoryAccess,
        path: &str,
        out: &'a mut [u8],
    ) -> Result<ObjectInfo<'a>, Status> {
        access.check(None, AccessRights::DESCRIBE)?;
        let used = if path.is_empty() {
            liboal::info::encode(&[liboal::DIRECTORY], &[], out)
                .map_err(|_| Status::SummaryTooLarge)?
        } else {
            let name = self.leaf_path(access, path, AccessRights::DESCRIBE)?;
            let named = self.names.get_by(name).ok_or(Status::NotFound)?;
            self.info(named.context, out)?
        };
        ObjectInfo::decode(&out[..used]).map_err(|_| Status::Internal)
    }
    pub fn enumerate<'a>(
        &self,
        access: &DirectoryAccess,
        path: &str,
        cursor: &[u8],
        max_entries: u32,
        out: &'a mut [u8],
    ) -> Result<liboal::protocol::Page<'a>, Status> {
        access.check(None, AccessRights::ENUMERATE)?;
        if max_entries == 0 || !path.is_empty() {
            if !path.is_empty() {
                self.leaf_path(access, path, AccessRights::ENUMERATE)?;
                return Err(Status::NotDirectory);
            }
            return Err(Status::Invalid);
        }
        let after = if cursor.is_empty() {
            ""
        } else {
            let mut r = Reader::new(cursor);
            if r.u64().map_err(|_| Status::CursorInvalid)? != self.incarnation {
                return Err(Status::CursorInvalid);
            }
            let name = r.text32().map_err(|_| Status::CursorInvalid)?;
            if !protocol::valid_name(name) {
                return Err(Status::CursorInvalid);
            }
            r.finish().map_err(|_| Status::CursorInvalid)?;
            name
        };
        if !after.is_empty() && !access.scope().permits(after) {
            return Err(Status::CursorInvalid);
        }
        let mut entries = [0u8; protocol::BODY_MAX];
        let mut writer = Writer::new(&mut entries);
        let mut last = after;
        let mut count = 0u32;
        let mut more = false;
        while let Some((name, entry)) =
            self.names
                .next_after::<str>(if last.is_empty() { None } else { Some(last) })
        {
            let name: &str = name.borrow();
            if !access.scope().permits(name) {
                last = name;
                continue;
            }
            let mut info = [0u8; protocol::BODY_MAX];
            let used = self.info(entry.context, &mut info)?;
            let item = 8usize
                .checked_add(name.len())
                .and_then(|n| n.checked_add(used))
                .ok_or(Status::EntryTooLarge)?;
            let full = 16usize
                .checked_add(12 + name.len())
                .and_then(|n| n.checked_add(writer.written()))
                .and_then(|n| n.checked_add(item))
                .ok_or(Status::EntryTooLarge)?;
            if count == max_entries || full > protocol::BODY_MAX {
                more = true;
                break;
            }
            writer.bytes32(name.as_bytes());
            writer.bytes32(&info[..used]);
            count += 1;
            last = name;
        }
        if more && count == 0 {
            return Err(Status::EntryTooLarge);
        }
        let used = writer.written();
        let mut opaque = [0u8; protocol::BODY_MAX];
        let opaque_len = if more {
            let mut w = Writer::new(&mut opaque);
            w.u64(self.incarnation);
            w.bytes32(last.as_bytes());
            w.written()
        } else {
            0
        };
        let len = 12usize
            .checked_add(opaque_len)
            .and_then(|n| n.checked_add(used))
            .ok_or(Status::EntryTooLarge)?;
        if out.len() < len || len + 4 > protocol::BODY_MAX {
            return Err(Status::EntryTooLarge);
        }
        let mut w = Writer::new(out);
        w.bytes32(&opaque[..opaque_len]);
        w.u32(u32::from(!more));
        w.u32(count);
        w.bytes(&entries[..used]);
        let used = w.written();
        liboal::protocol::Page::decode(&out[..used]).map_err(|_| Status::Internal)
    }
    pub fn mark_control_closed(&mut self, context: u64) -> Result<(), Status> {
        self.withdraw(context, Reason::ControlClosed)?;
        self.registrations
            .get_mut(context)
            .ok_or(Status::Permission)?
            .control_closed = true;
        Ok(())
    }
    pub fn endpoint_source_removed(&mut self, context: u64) {
        if let Some(record) = self.registrations.get_mut(context) {
            record.endpoint_removed = true;
        }
    }
    pub fn control_source_removed(&mut self, context: u64) {
        if let Some(record) = self.registrations.get_mut(context) {
            record.control_removed = true;
        }
    }
    /// 来源解除后尝试释放 endpoint；失败原样恢复，调用者轮转继续，不向上传 Err。
    pub fn retire_one(&mut self, context: u64) -> bool {
        let Some(record) = self.registrations.get_mut(context) else {
            return true;
        };
        if record.info.state != State::Withdrawn {
            return false;
        }
        if !record.endpoint_removed {
            return false;
        }
        if let Some(body) = record.body.as_mut()
            && let Some(endpoint) = body.endpoint.take()
            && let Err((endpoint, _)) = endpoint.close()
        {
            body.endpoint = Some(endpoint);
            return false;
        }
        record.body = None;
        if record.control_closed && record.control_removed {
            self.registrations.remove(context);
            return true;
        }
        false
    }
    pub fn needs_endpoint_remove(&self, context: u64) -> bool {
        self.registrations
            .get(context)
            .is_some_and(|r| r.info.state == State::Withdrawn && !r.endpoint_removed)
    }
    pub fn retiring_task(&self, context: u64) -> Option<u64> {
        self.registrations
            .get(context)
            .filter(|r| r.info.state == State::Withdrawn && !r.endpoint_removed && r.task != 0)
            .map(|r| r.task)
    }
    pub fn is_retired(&self, context: u64) -> bool {
        self.registrations.get(context).is_none()
    }
    pub fn seal(&mut self) {
        self.sealed = true;
        self.watches.seal();
    }
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty() && self.names.is_empty() && self.watches.is_empty()
    }
    pub fn next_context(&self, after: Option<u64>) -> Option<u64> {
        self.registrations
            .next_after(after.as_ref())
            .map(|(&context, _)| context)
    }
    #[expect(
        clippy::result_large_err,
        reason = "退休未闭合时原样返还唯一 backing owner，不增加失败路径分配"
    )]
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        if self.is_empty() {
            // SAFETY: 两个 ManuallyDrop 都未提前释放，空表没有注册或入口 owner。
            unsafe {
                ManuallyDrop::drop(&mut self.registrations);
                ManuallyDrop::drop(&mut self.names);
            }
        }
    }
}
